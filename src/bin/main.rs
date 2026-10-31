#![no_std]
#![no_main]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use static_cell::StaticCell;

mod buffer;
mod refresh;
mod ssd1681;

use buffer::{FrameBuffer, hline, invert, vline};
use refresh::{Refresher, diff};
use ssd1681::Epd;

esp_bootloader_esp_idf::esp_app_desc!();

/// 调试日志总开关：定位完成后改 false 可一键静音
const LOG: bool = true;

pub(crate) const SQ: u16 = 16; // 方块边长
pub(crate) const Y0: u16 = 92; // 方块行带

/// 方块右移一格（到最右侧后回到最左）。修改了哪些像素由 diff 自动计算，回到起点的情形不需要单独处理
pub(crate) fn step_square(fb: &mut FrameBuffer, sx: &mut u16) {
    let new = if *sx + 2 * SQ > 192 { 8 } else { *sx + SQ };
    for yy in Y0..Y0 + SQ {
        for xx in *sx..*sx + SQ {
            fb.set_pixel(xx, yy, true); // 擦旧
        }
        for xx in new..new + SQ {
            fb.set_pixel(xx, yy, false); // 画新
        }
    }
    *sx = new;
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let cs = Output::new(peripherals.GPIO10, Level::High, OutputConfig::default());
    let dc = Output::new(peripherals.GPIO5, Level::Low, OutputConfig::default());
    let rst = Output::new(peripherals.GPIO4, Level::High, OutputConfig::default());
    let busy = Input::new(peripherals.GPIO3, InputConfig::default());

    let spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default().with_frequency(Rate::from_mhz(1)),
    )
    .expect("SPI 配置失败")
    .with_sck(peripherals.GPIO6)
    .with_mosi(peripherals.GPIO7);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    let mut epd = Epd {
        spi,
        cs,
        dc,
        rst,
        busy,
    };
    epd.init().await;

    // 两个大对象都住 StaticCell（内存纪律）
    static FB: StaticCell<FrameBuffer> = StaticCell::new();
    let fb = FB.init(FrameBuffer::new_white());
    static RF: StaticCell<Refresher> = StaticCell::new();
    let refresh = RF.init(Refresher::new(epd)); // 拿到的是 &'static mut，方法调用会自动可变借用，绑定本身不用 mut

    // ===== diff 自检：纯逻辑，不碰硬件，先验证算法正确 =====
    assert!(diff(fb, fb).is_none(), "全同帧应得 None");
    fb.set_pixel(19, 9, false); // 落在字节 2（x 16..23）、行 9
    fb.set_pixel(100, 50, false); // 落在字节 12（x 96..103）、行 50
    let (r, pct) = diff(fb, &refresh.shadow).unwrap();
    assert_eq!((r.x, r.y, r.w, r.h), (16, 9, 88, 42), "脏区包围盒算错");
    assert_eq!(pct, 0, "两个像素的占比应为 0%");
    esp_println::println!("diff 自检通过");
    *fb = FrameBuffer::new_white(); // 恢复白板，进正式演示

    // ===== 开场图：边框 + 起点方块，第一帧 present 走 R2 立基准 =====
    hline(fb, 0, 199, 0);
    hline(fb, 0, 199, 199);
    vline(fb, 0, 0, 199);
    vline(fb, 199, 0, 199);
    let mut sx: u16 = 8;
    for yy in Y0..Y0 + SQ {
        for xx in sx..sx + SQ {
            fb.set_pixel(xx, yy, false);
        }
    }
    refresh.present(fb).await; // R2：全刷立基准

    // ===== 演示循环：四种决策各看一眼 =====
    loop {
        // A) 整屏反色：100% 变更 → 直接转全刷
        Timer::after(Duration::from_millis(700)).await;
        invert(fb);
        refresh.present(fb).await;
        Timer::after(Duration::from_millis(1_500)).await;
        invert(fb); // 反回来，又一次全刷
        refresh.present(fb).await;

        // B) 滑 11 步：前 10 步局刷积累次数，第 11 步达到 R9 上限 → 全刷重建并清除残影
        for _ in 0..11 {
            Timer::after(Duration::from_millis(700)).await;
            step_square(fb, &mut sx);
            refresh.present(fb).await;
        }

        // C) 同一帧再推一次 → 跳过（对照日志，屏应没有任何变化）
        Timer::after(Duration::from_millis(700)).await;
        refresh.present(fb).await;
    }
}
