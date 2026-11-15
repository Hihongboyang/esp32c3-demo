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

use buffer::FrameBuffer;
use refresh::{Refresher, diff};
use ssd1681::Epd;

use embedded_graphics::mono_font::{
    MonoTextStyle,
    ascii::{FONT_6X12, FONT_10X20},
};
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::text::{Baseline, Text};

esp_bootloader_esp_idf::esp_app_desc!();

/// 调试日志总开关：定位完成后改 false 可一键静音
const LOG: bool = true;

/// 醒着的拍数（每拍 1s）与深睡时长——装置的节拍器
const AWAKE_TICKS: u32 = 8;
const SLEEP_SECS: u64 = 60;

/// RTC 快速内存：深睡也保电。persistent = 首次上电清零，
/// 之后重启/深睡唤醒都不再初始化——内容原样保留
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut MAGIC: u32 = 0; // 魔数：区分「真数据」和「上电随机值」
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut WAKE_COUNT: u32 = 0; // 第几次醒（首次上电算第 1 次）
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut AWAKE_SECS: u32 = 0; // 历次会话累计醒着的秒数

// ===== 应用层（L4）：页面状态机 =====
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

/// 装置页：第几次醒 + 累计醒时。版式 = L10 STATS 页同款
fn render_wake(fb: &mut FrameBuffer, wake: u32, awake: u32, small: MonoTextStyle<'_, BinaryColor>) {
    let line = MonoTextStyle::new(&FONT_10X20, BinaryColor::Off);
    Text::with_baseline("DEVICE", Point::new(10, 10), small, Baseline::Top)
        .draw(fb)
        .ok();
    let (mut b1, mut b2) = ([b'0'; 5], [b'0'; 5]);
    Text::with_baseline("WAKE", Point::new(10, 60), line, Baseline::Top)
        .draw(fb)
        .ok();
    Text::with_baseline(
        fmt_secs(&mut b1, wake),
        Point::new(80, 60),
        line,
        Baseline::Top,
    )
    .draw(fb)
    .ok();
    Text::with_baseline("AWAKE", Point::new(10, 95), line, Baseline::Top)
        .draw(fb)
        .ok();
    Text::with_baseline(
        fmt_secs(&mut b2, awake),
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

    // ===== 装置会话：main() 的一次完整运行 = 一次「醒」 =====
    let header = MonoTextStyle::new(&FONT_6X12, BinaryColor::Off);

    // 恢复跨睡眠状态：魔数对上 = 深睡唤醒（RTC 内存还在）；对不上 = 冷启动（清零重来）
    let (wake, awake0) = if unsafe { MAGIC } == 0x00C0_FFEE {
        unsafe { WAKE_COUNT += 1 };
        (unsafe { WAKE_COUNT }, unsafe { AWAKE_SECS })
    } else {
        unsafe {
            MAGIC = 0x00C0_FFEE;
            WAKE_COUNT = 1;
            AWAKE_SECS = 0;
        }
        (1, 0)
    };
    if LOG {
        esp_println::println!("=== 第 {wake} 次醒（累计醒 {awake0}s）===");
    }

    // 醒着的 8 拍：首拍走 R2 全刷立基准（重启后 base_ready=false），之后局刷只动数字
    for i in 0..AWAKE_TICKS {
        fb.clear_white();
        render_wake(fb, wake, awake0 + i, header);
        refresh.present(fb).await;
        Timer::after(Duration::from_millis(1_000)).await;
    }

    // 睡前三件事：保存累计 → 屏先睡（画面保持）→ C3 深睡（不返回）
    unsafe { AWAKE_SECS = awake0 + AWAKE_TICKS };
    if LOG {
        esp_println::println!("=== 屏深睡 + C3 深睡 {SLEEP_SECS}s ===");
    }
    refresh.hibernate_screen().await;
    use esp_hal::rtc_cntl::sleep::{LowPower, RtcSleepConfig};
    use esp_hal::time::{Duration as HalDuration, Instant as HalInstant};
    let mut lp = LowPower::new(peripherals.LPWR); // LPWR 先认领到变量，同 cs/rst 的拿法
    lp.set_wakeup_deadline(HalInstant::now() + HalDuration::from_secs(SLEEP_SECS));
    lp.sleep_deep(RtcSleepConfig::deep()); // 不返回：唤醒=芯片复位重启，从 main 开头再来
}
