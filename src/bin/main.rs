#![no_std]
#![no_main]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
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

use buffer::FrameBuffer;
use refresh::{Refresher, diff};
use ssd1681::Epd;

use embedded_graphics::mono_font::{
    MonoTextStyle,
    ascii::{FONT_6X12, FONT_10X20},
};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};

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

// ===== 应用层（L4）：页面状态机 =====

/// 装置当前显示哪幅画面。enum = 多选一：同一时刻只处于一个变体
enum Page {
    Uptime,
    Stats,
}

/// 秒数 → 固定 5 字符右对齐（前导空格，末位总保留），如 " 42"
fn fmt_secs(buf: &mut [u8; 5], secs: u32) -> &str {
    let mut v = secs;
    for b in buf.iter_mut().rev() {
        // 从个位往高位填
        *b = b'0' + (v % 10) as u8;
        v /= 10;
    }
    let n = buf.len() - 1;
    let (head, _last) = buf.split_at_mut(n); // 末位总保留：0 显示 "    0"
    for b in head {
        if *b == b'0' {
            *b = b' ';
        } else {
            break;
        }
    }
    core::str::from_utf8(buf).unwrap() // 全 ASCII，转换不会失败
}

/// 页面一：运行秒数
fn render_uptime(fb: &mut FrameBuffer, secs: u32, small: MonoTextStyle<'_, BinaryColor>) {
    let big = MonoTextStyle::new(&FONT_10X20, BinaryColor::Off);
    Text::with_baseline("UPTIME", Point::new(10, 10), small, Baseline::Top)
        .draw(fb)
        .ok();
    let mut b = [b'0'; 5];
    Text::with_baseline(
        fmt_secs(&mut b, secs),
        Point::new(75, 80),
        big,
        Baseline::Top,
    )
    .draw(fb)
    .ok();
}

/// 页面二：刷新统计
fn render_stats(
    fb: &mut FrameBuffer,
    partials: u32,
    fulls: u32,
    small: MonoTextStyle<'_, BinaryColor>,
) {
    let line = MonoTextStyle::new(&FONT_10X20, BinaryColor::Off);
    Text::with_baseline("STATS", Point::new(10, 10), small, Baseline::Top)
        .draw(fb)
        .ok();
    let (mut b1, mut b2) = ([b'0'; 5], [b'0'; 5]);
    Text::with_baseline("PART", Point::new(10, 60), line, Baseline::Top)
        .draw(fb)
        .ok();
    Text::with_baseline(
        fmt_secs(&mut b1, partials),
        Point::new(80, 60),
        line,
        Baseline::Top,
    )
    .draw(fb)
    .ok();
    Text::with_baseline("FULL", Point::new(10, 95), line, Baseline::Top)
        .draw(fb)
        .ok();
    Text::with_baseline(
        fmt_secs(&mut b2, fulls),
        Point::new(80, 95),
        line,
        Baseline::Top,
    )
    .draw(fb)
    .ok();
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

    // ===== 应用层主循环：每秒一帧，5 秒一换页 =====
    let boot = Instant::now();
    let header = MonoTextStyle::new(&FONT_6X12, BinaryColor::Off);
    let mut page = Page::Uptime;
    let mut tick: u32 = 0;
    loop {
        Timer::after(Duration::from_millis(1_000)).await;
        tick += 1;
        if tick % 5 == 0 {
            page = match page {
                Page::Uptime => Page::Stats,
                Page::Stats => Page::Uptime,
            };
        }
        // 整帧重组：从白底重画当前页面；变化多少由 diff 说了算
        fb.clear_white();
        match page {
            Page::Uptime => render_uptime(fb, boot.elapsed().as_secs() as u32, header),
            Page::Stats => {
                let (partials, fulls) = refresh.stats();
                render_stats(fb, partials, fulls, header);
            }
        }
        refresh.present(fb).await;
    }
}
