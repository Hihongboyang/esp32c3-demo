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

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // ===== 控制线（对照接线表） =====
    let mut cs = Output::new(peripherals.GPIO10, Level::High, OutputConfig::default()); // 高=不选中
    let mut dc = Output::new(peripherals.GPIO5, Level::Low, OutputConfig::default()); // 低=命令
    let mut rst = Output::new(peripherals.GPIO4, Level::High, OutputConfig::default()); // 低=复位

    // BUSY：高=忙（屏数据手册为准）。屏自己驱动这根线，不用上拉
    let busy = Input::new(peripherals.GPIO3, InputConfig::default());

    // ===== SPI 主机 =====
    // 屏要求 Mode 0（默认即 Mode 0，正合适）；首通用 100 kHz 求稳，数据手册上限 20 MHz
    // esp-hal 1.2.1：Spi::new 返回 Result，要接住
    let mut spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default().with_frequency(Rate::from_khz(100)),
    )
    .expect("SPI 配置失败")
    .with_sck(peripherals.GPIO6)
    .with_mosi(peripherals.GPIO7);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    esp_println::println!("=== 墨水屏首通 ===");

    // 上电给屏一点自举时间
    Timer::after(Duration::from_millis(50)).await;
    esp_println::println!("复位前 BUSY = {}", busy.is_high());

    // 硬件复位序列（照抄官方 demo）：高 20ms → 低 5ms → 高 20ms
    rst.set_high();
    Timer::after(Duration::from_millis(20)).await;
    rst.set_low();
    Timer::after(Duration::from_millis(5)).await;
    rst.set_high();
    Timer::after(Duration::from_millis(20)).await;
    wait_idle(&busy).await;

    // ===== 用 SPI 说第一句话：软复位命令 0x12 =====
    cs.set_low(); // 选中屏：整个命令块期间保持低
    dc.set_low(); // 接下来是命令字节
    spi.write(&[0x12]).expect("SPI 写入失败");
    cs.set_high(); // 说完释放

    esp_println::println!("已发送软复位 0x12");
    wait_idle(&busy).await;
    esp_println::println!("握手成功：SPI 无错 + BUSY 可读。屏活着。");

    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}

/// 等屏忙完：BUSY 高=忙。有限等待（最多 3 秒），绝不死等
async fn wait_idle(busy: &Input<'_>) {
    let mut waited = 0u32;
    while busy.is_high() {
        if waited >= 3_000 {
            esp_println::println!("BUSY 超时 3 秒——接线或极性有问题，别硬等");
            return;
        }
        Timer::after(Duration::from_millis(10)).await;
        waited += 10;
    }
    esp_println::println!("BUSY 空闲（等了 {} ms）", waited);
}