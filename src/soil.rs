//! D2-5-2K 土壤湿度传感器驱动(模拟输出 → ADC)
//!
//! 模块上的 NE555 把探针阻抗变化转成模拟电压。这里只做两件事:
//! 多次采样取平均、按两点标定换算成湿度百分比。

use esp_hal::analog::adc::{Adc, AdcChannel, AdcCalLine, AdcConfig, AdcPin, Attenuation};
use esp_hal::gpio::AnalogPin;
use esp_hal::Blocking;

use crate::Adc1;

/// 两点标定(请按自己的模块实测修改):
/// - `DRY`:探针放在干燥空气中读取的原始值
/// - `WET`:探针插入水中读取的原始值
///
/// 本模块(NE555 方案)通常湿度越大电压越高、读数越大;
/// 若你的版本方向相反,把两个常量对调即可。
pub const DRY: u16 = 2780;
pub const WET: u16 = 1120;

pub struct SoilSensor<PIN> {
    pin: AdcPin<PIN, Adc1, AdcCalLine<Adc1>>,
}

impl<PIN: AdcChannel + AnalogPin> SoilSensor<PIN> {
    /// 把引脚配置为模拟输入并启用 eFuse 线性校准,11dB 衰减量程约 0~3.1V。
    pub fn new(config: &mut AdcConfig<Adc1>, pin: PIN) -> Self {
        let pin =
            config.enable_pin_with_cal::<_, AdcCalLine<Adc1>>(pin, Attenuation::_11dB);
        Self { pin }
    }

    /// 采样 `samples` 次取平均。555 输出经整流滤波后仍有纹波,
    /// 加上 ADC 本身的噪声,单次读数会抖动,平均后稳定得多。
    // 主程序使用;soil-test 单独编译本文件时未用到,避免误报
    #[allow(dead_code)]
    pub fn read_avg(&mut self, adc: &mut Adc<'_, Adc1, Blocking>, samples: u32) -> u16 {
        self.read_burst(adc, samples).avg
    }

    /// 采样一个窗口并给出诊断统计,用于观察读数质量:
    /// 单次值与均值的差、窗口内 min..max 的跨度(纹波 + 噪声幅度)。
    pub fn read_burst(&mut self, adc: &mut Adc<'_, Adc1, Blocking>, samples: u32) -> Burst {
        let samples = samples.max(1);
        let (mut sum, mut min, mut max, mut first) = (0u32, u16::MAX, 0u16, 0u16);
        for i in 0..samples {
            let value = nb::block!(adc.read_oneshot(&mut self.pin))
                .expect("ADC 转换不会返回硬错误");
            if i == 0 {
                first = value;
            }
            sum += value as u32;
            min = min.min(value);
            max = max.max(value);
        }
        Burst {
            first,
            avg: (sum / samples) as u16,
            min,
            max,
        }
    }
}

/// 一个采样窗口的诊断结果。
// 字段由 soil-test 读取;主程序只用 avg,单独编译时会误报字段未读
#[allow(dead_code)]
pub struct Burst {
    /// 窗口第一个原始值,用来看单次读数偏离均值多远。
    pub first: u16,
    /// 窗口平均,`read_avg` 返回的就是它。
    pub avg: u16,
    /// 窗口最小/最大,两者之差是纹波 + 噪声的综合幅度。
    pub min: u16,
    pub max: u16,
}

/// 按两点标定把原始 ADC 值换算为 0~100 的湿度百分比。
pub fn raw_to_percent(raw: u16) -> f32 {
    let value = (raw as i32 - DRY as i32) as f32 / (WET as i32 - DRY as i32) as f32 * 100.0;
    value.clamp(0.0, 100.0)
}