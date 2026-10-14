//! D2-5-2K 土壤湿度传感器 单传感器测试（esp-rtos 异步风格，与课程模板同款）
//!
//! 只读这一个传感器,逐秒打印一行诊断数据,用于:
//! - 验证接线与 ADC 通路是否正常
//! - 观察波形:NE555 方案的模块泡水后频率塌到 ~0.15Hz,AOUT 变成慢方波,
//!   「单次/窗口/波动」三列就是它的示波器
//! - 两点标定:读「慢均」列(EMA,约 1 分钟收敛)——晾干记 DRY、泡水记 WET,
//!   回填 `src/soil.rs` 顶部常量
//!
//! ## 接线
//!
//! ```text
//! D2-5-2K         ESP32-C3
//!   VCC  --------  5V   (V1.2 版本必须 5V;V2.0 版本 3.3V/5V 均可)
//!   GND  --------  GND
//!   AOUT --分压--  GPIO4 (ADC1_CH4)
//! ```
//!
//! ⚠️ V1.2 版本没有 5V→3V 稳压,AOUT 可超 3.3V,必须电阻分压后才能接 GPIO4
//! (如 AOUT—[10kΩ]—GPIO4—[20kΩ]—GND,分压比 2/3);V2.0 板载稳压,AOUT ≤ 3V 可直连。
//! 版本判断看模块丝印或 `docs/D2-5-2K/` 下的原理图。
//!
//! ⚠️ 若墨水屏(epd-c3 课程)还接在同一块板上:GPIO4 是屏的 RST 线,
//! 测传感器前先把屏断开,两边别同时占这个脚。
//!
//! 运行:`cargo run --bin soil-test --release`(串口 115200)

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::analog::adc::{Adc, AdcConfig};
use esp_hal::clock::CpuClock;
use esp_hal::peripherals::ADC1;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;

// 复用主工程的驱动;soil.rs 里 `use crate::Adc1`,这里提供同一定义
#[path = "../soil.rs"]
mod soil;

// 固件描述段:给 esp-idf bootloader 看的"自我介绍",模板标配
esp_bootloader_esp_idf::esp_app_desc!();

/// 每行统计的采样窗口大小(毫秒级快照,看波形/纹波用)
const SAMPLES: u32 = 32;
/// 慢均值时间常数:每秒吸收新数据的 1/EMA_N,泡水时的方波(周期约 6~8 秒)
/// 大约一分钟内收敛到占空比平均。
const EMA_N: u32 = 16;
/// 打印周期
const PERIOD: Duration = Duration::from_millis(1_000);
/// 每隔多少行重印一次标定提示
const HINT_EVERY: u32 = 15;
/// 湿度百分比条图的格数
const BAR_LEN: usize = 20;

/// 供 soil.rs 使用的便捷别名(与主程序 main.rs 保持一致)
pub(crate) type Adc1 = ADC1<'static>;

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // TIMG0 硬件定时器 + 软件中断交给 esp-rtos 当"心跳"(调度器的时间来源)。
    // 必须在第一次 Timer::after 之前 start——课程 L2 踩过的坑。
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    println!("\r\n=== D2-5-2K 土壤湿度传感器测试(GPIO4 / ADC1_CH4)===");
    println!("泡水时输出是 ~0.15Hz 方波:「窗口/波动」看波形,「慢均」才是湿度读数");
    println!("标定读「慢均」:晾干 → DRY;泡水跑稳 1 分钟 → WET;回填 src/soil.rs\r");

    // GPIO4 配为模拟输入 + eFuse 线性校准
    let mut adc_config = AdcConfig::new();
    let mut soil = soil::SoilSensor::new(&mut adc_config, peripherals.GPIO4);
    let mut adc1 = Adc::new(peripherals.ADC1, adc_config);

    let mut count: u32 = 0;
    let mut ema: Option<f32> = None;
    loop {
        // ADC 单次转换是微秒级,阻塞读完即走;
        // 周期性的"等"交给 Timer::after——期间让出执行权,不占着 CPU 空转
        //
        // 每行两层数据:
        // - read_burst:毫秒级快照,看纹波/波形用(泡水时它是 0↔满幅的方波相位,天然会跳);
        // - ema:慢速滑动均值(每秒吸收 1/EMA_N),泡水时收敛到方波的占空比平均——
        //   标定和湿度读数都用它。
        let b = soil.read_burst(&mut adc1, SAMPLES);
        match ema {
            Some(ref mut e) => *e += (b.avg as f32 - *e) / EMA_N as f32,
            None => ema = Some(b.avg as f32), // 首行直接锚定,避免从 0 爬坡
        }
        let slow = ema.unwrap_or(0.0) as u16;

        let percent = soil::raw_to_percent(slow);
        let volt = slow as f32 / 4095.0 * 3.1;

        let mut bar = [b'.'; BAR_LEN];
        let filled = (percent as usize * BAR_LEN).div_ceil(100);
        for slot in bar.iter_mut().take(filled.min(BAR_LEN)) {
            *slot = b'#';
        }
        let bar = core::str::from_utf8(&bar).unwrap_or("");

        println!(
            "[{count:>4}] 单次 {:4} | 窗口 {:4}..{:<4} 波动 {:3} | 慢均 {slow:4} ≈{volt:.2} V | {percent:3.0} % |{bar}|",
            b.first,
            b.min,
            b.max,
            b.max - b.min,
        );

        count += 1;
        if count.is_multiple_of(HINT_EVERY) {
            println!(
                "— 标定 DRY={} WET={}:读「慢均」列(晾干→DRY,泡水→WET),回填 src/soil.rs 顶部",
                soil::DRY,
                soil::WET
            );
        }
        Timer::after(PERIOD).await;
    }
}
