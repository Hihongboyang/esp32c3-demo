#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::Blocking;

esp_bootloader_esp_idf::esp_app_desc!();

// ================= 数据区：照抄官方 demo（老师提供，勿手打） =================

/// 全刷波形表 WF_Full_1IN54：159 字节，逐字节来自 epd1in54_V2.cpp 第 35–56 行
const WF_FULL: [u8; 159] = [
    0x80, 0x48, 0x40, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x40, 0x48, 0x80, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x80, 0x48, 0x40, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x40, 0x48, 0x80, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0xA, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x8, 0x1, 0x0, 0x8, 0x1, 0x0, 0x2,
    0xA, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x22, 0x22, 0x22, 0x22, 0x22, 0x22, 0x0, 0x0, 0x0,
    0x22, 0x17, 0x41, 0x0, 0x32, 0x20,
];

/// 一整帧"全白"：5000 字节全 1（1=白）。静态放 Flash，不占栈
static WHITE_FRAME: [u8; 5_000] = [0xFF; 5_000];

// ================= 驱动：SSD1681 门面（DESIGN L1 层雏形） =================

struct Epd {
    spi: Spi<'static, Blocking>,
    cs:  Output<'static>,
    dc:  Output<'static>,
    rst: Output<'static>,
    busy: Input<'static>,
}

impl Epd {
    /// 一个完整命令块：CS 拉低 → DC 低发命令字节 → DC 高发数据 → CS 拉高
    fn cmd(&mut self, command: u8, data: &[u8]) {
        self.cs.set_low();
        self.dc.set_low();
        self.spi.write(&[command]).expect("SPI 写命令失败");
        if !data.is_empty() {
            self.dc.set_high();
            self.spi.write(data).expect("SPI 写数据失败");
        }
        self.cs.set_high();
    }

    /// 硬件复位（官方序列）：高 20ms → 低 5ms → 高 20ms
    async fn reset(&mut self) {
        self.rst.set_high();
        Timer::after(Duration::from_millis(20)).await;
        self.rst.set_low();
        Timer::after(Duration::from_millis(5)).await;
        self.rst.set_high();
        Timer::after(Duration::from_millis(20)).await;
    }

    /// 等忙结束：BUSY 高=忙；有限等待；空闲后照 demo 再稳 200ms
    async fn wait_idle(&mut self, timeout_ms: u32) -> bool {
        let mut waited = 0u32;
        while self.busy.is_high() {
            if waited >= timeout_ms {
                esp_println::println!("BUSY 超时 {}ms——查接线", timeout_ms);
                return false;
            }
            Timer::after(Duration::from_millis(10)).await;
            waited += 10;
        }
        Timer::after(Duration::from_millis(200)).await; // demo 的严谨尾巴
        true
    }

    /// 装载波形表：0x32 收 153 字节，后 6 字节进 4 个配套寄存器
    async fn set_lut(&mut self, lut: &[u8; 159]) {
        self.cmd(0x32, &lut[..153]);
        self.wait_idle(3_000).await;
        self.cmd(0x3F, &lut[153..154]);
        self.cmd(0x03, &lut[154..155]);
        self.cmd(0x04, &lut[155..158]);
        self.cmd(0x2C, &lut[158..159]);
    }

    /// 初始化：直译官方 demo HDirInit()
    async fn init(&mut self) {
        self.reset().await;
        self.wait_idle(3_000).await;
        self.cmd(0x12, &[]);                     // 软复位
        self.wait_idle(3_000).await;

        self.cmd(0x01, &[0xC7, 0x00, 0x01]);     // 驱动输出：199 行，扫描方向
        self.cmd(0x11, &[0x01]);                 // 数据入口：X 增、Y 增
        self.cmd(0x44, &[0x00, 0x18]);           // RAM X 窗口 0~24 字节 = 200 像素
        self.cmd(0x45, &[0xC7, 0x00, 0x00, 0x00]); // RAM Y 窗口 0~199 行
        self.cmd(0x3C, &[0x01]);                 // 边框波形：跟白走
        self.cmd(0x18, &[0x80]);                 // 温度：用内部传感器
        self.cmd(0x22, &[0xB1]);                 // 载入温度
        self.cmd(0x20, &[]);
        self.wait_idle(3_000).await;
        self.cmd(0x4E, &[0x00]);                 // 写指针 X 归零
        self.cmd(0x4F, &[0xC7, 0x00]);           // 写指针 Y 归零
        self.wait_idle(3_000).await;
        self.set_lut(&WF_FULL).await;
    }

    /// 全刷：0x22 0xC7 组合开关 → 0x20 点火 → 等忙（标称 ~2s，给 4 倍余量）
    async fn refresh_full(&mut self) {
        self.cmd(0x22, &[0xC7]);
        self.cmd(0x20, &[]);
        self.wait_idle(8_000).await;
    }

    /// 清屏成全白（demo Clear()）：两个帧面都写白 + 全刷
    async fn clear_white(&mut self) {
        self.cmd(0x24, &WHITE_FRAME);   // 新画面：全白
        self.cmd(0x26, &WHITE_FRAME);   // 第二帧面也写白（局刷基准，第 6 课用）
        self.refresh_full().await;
    }
}

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let cs = Output::new(peripherals.GPIO10, Level::High, OutputConfig::default());
    let dc = Output::new(peripherals.GPIO5, Level::Low, OutputConfig::default());
    let rst = Output::new(peripherals.GPIO4, Level::High, OutputConfig::default());
    let busy = Input::new(peripherals.GPIO3, InputConfig::default());

    // 5000 字节 × 2 帧：1 MHz 下约 80ms 一轮，比上节课的 100kHz 舒服
    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default().with_frequency(Rate::from_mhz(1)),
    )
    .expect("SPI 配置失败")
    .with_sck(peripherals.GPIO6)
    .with_mosi(peripherals.GPIO7);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    let mut epd = Epd { spi, cs, dc, rst, busy };

    esp_println::println!("=== 第 4 课：屏幕点亮时刻 ===");
    epd.init().await;
    esp_println::println!("init 完成，全刷波形表已装载");

    esp_println::println!("开始清屏……盯住屏！");
    epd.clear_white().await;
    esp_println::println!("完成。屏现在应该是纯白。");

    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}