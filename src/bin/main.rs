#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::timer::timg::TimerGroup;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

const DEBOUNCE: Duration = Duration::from_millis(20);
const POLL: Duration = Duration::from_millis(1);
const COMBO_WINDOW: Duration = Duration::from_millis(2000); // 连击判定窗口
const COMBO_THRESHOLD: u32 = 5; // 窗口内按满几次触发
const FLASH_TIMES: u32 = 100;

/// 等待一次有效按下（低电平稳定保持 DEBOUNCE，滤除抖动）
async fn wait_press(button: &Input<'_>) {
    loop {
        if button.is_low() {
            Timer::after(DEBOUNCE).await;
            if button.is_low() {
                return;
            }
        }
        Timer::after(POLL).await;
    }
}

/// 等待一次有效松开（高电平稳定保持 DEBOUNCE）
async fn wait_release(button: &Input<'_>) {
    loop {
        if button.is_high() {
            Timer::after(DEBOUNCE).await;
            if button.is_high() {
                return;
            }
        }
        Timer::after(POLL).await;
    }
}

/// 快速翻转 LED n 次
async fn flash(led: &mut Output<'_>, times: u32) {
    for _ in 0..times {
        led.toggle();
        Timer::after(Duration::from_millis(20)).await;
    }
}

/// 连击计数：相邻两次按下间隔在窗口内则累计，达到阈值触发
struct Combo {
    streak: u32,
    last: Instant,
}

impl Combo {
    fn new() -> Self {
        // streak 从 0 开始：首次按下无论距开机多久都计为第 1 次
        Self {
            streak: 0,
            last: Instant::now(),
        }
    }

    /// 记录一次按下，返回连击是否达到阈值
    fn press(&mut self) -> bool {
        let now = Instant::now();
        self.streak = if now - self.last <= COMBO_WINDOW {
            self.streak + 1
        } else {
            1 // 超过窗口，连击重新计数
        };
        self.last = now;
        self.streak >= COMBO_THRESHOLD
    }

    fn reset(&mut self) {
        self.streak = 0;
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let mut led = Output::new(peripherals.GPIO8, Level::High, OutputConfig::default());

    let boot_button = Input::new(
        peripherals.GPIO9,
        InputConfig::default().with_pull(Pull::Up),
    );

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    let mut count = 0u32;
    let mut combo = Combo::new();

    loop {
        wait_press(&boot_button).await;

        count += 1;
        led.toggle();
        esp_println::println!("boot button pressed: {}", count);

        if combo.press() {
            flash(&mut led, FLASH_TIMES).await;
            combo.reset();
        }

        wait_release(&boot_button).await;
    }
}
