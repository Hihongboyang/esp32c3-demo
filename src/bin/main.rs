#![no_std]
#![no_main]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::Blocking;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig};
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

// ===== 波形表：与第 4 课相同，照抄 demo（老师提供，勿手打） =====
const WF_FULL: [u8; 159] = [
    0x80, 0x48, 0x40, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x40, 0x48, 0x80, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x80, 0x48, 0x40, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x40, 0x48, 0x80, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0xA, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x8, 0x1, 0x0, 0x8, 0x1, 0x0,
    0x2, 0xA, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x22, 0x22, 0x22, 0x22,
    0x22, 0x22, 0x0, 0x0, 0x0, 0x22, 0x17, 0x41, 0x0, 0x32, 0x20,
];
// ===== 局刷波形表（demo 的 WF_PARTIAL_1IN54_0，老师提供，勿手打） =====
const WF_PARTIAL: [u8; 159] = [
    0x0, 0x40, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x80, 0x80, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x40, 0x40, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x80, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0xF, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x1, 0x1, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x22, 0x22, 0x22, 0x22, 0x22,
    0x22, 0x0, 0x0, 0x0, 0x2, 0x17, 0x41, 0xB0, 0x32, 0x28,
];

// ===== L2 层开工：帧缓冲（DESIGN §2.4） =====

/// 5000 字节画布。不变式：坐标永远以"代码约定方向"存储，1=白 0=黑，MSB 在前
struct FrameBuffer([u8; 5_000]);

impl FrameBuffer {
    fn new_white() -> Self {
        Self([0xFF; 5_000])
    }

    /// x、y ∈ 0..199。white=true 画白点，false 画黑点
    fn set_pixel(&mut self, x: u16, y: u16, white: bool) {
        let byte = (y as usize) * 25 + (x >> 3) as usize;
        let bit = 7 - (x & 7); // MSB 在前：x=0 用 bit7
        if white {
            self.0[byte] |= 1 << bit; // 置 1 = 白
        } else {
            self.0[byte] &= !(1 << bit); // 清 0 = 黑
        }
    }
}

/// 水平线（闭区间 [x0, x1]）
fn hline(fb: &mut FrameBuffer, x0: u16, x1: u16, y: u16) {
    for x in x0..=x1 {
        fb.set_pixel(x, y, false);
    }
}

/// 垂直线（闭区间 [y0, y1]）
fn vline(fb: &mut FrameBuffer, x: u16, y0: u16, y1: u16) {
    for y in y0..=y1 {
        fb.set_pixel(x, y, false);
    }
}

/// Bresenham 直线：纯整数运算画任意方向线段（1965 年的算法，嵌入式至今在用）
fn line(fb: &mut FrameBuffer, x0: i32, y0: i32, x1: i32, y1: i32) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let (mut x, mut y, mut err) = (x0, y0, dy + dx);
    loop {
        fb.set_pixel(x as u16, y as u16, false);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

// ===== L1 层驱动：第 4 课的 Epd + 新增 push_frame =====

struct Epd {
    spi: Spi<'static, Blocking>,
    cs: Output<'static>,
    dc: Output<'static>,
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
        Timer::after(Duration::from_millis(200)).await;
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
        self.cmd(0x12, &[]);
        self.wait_idle(3_000).await;
        self.cmd(0x01, &[0xC7, 0x00, 0x01]);
        self.cmd(0x11, &[0x01]);
        self.cmd(0x44, &[0x00, 0x18]);
        self.cmd(0x45, &[0xC7, 0x00, 0x00, 0x00]);
        self.cmd(0x3C, &[0x01]);
        self.cmd(0x18, &[0x80]);
        self.cmd(0x22, &[0xB1]);
        self.cmd(0x20, &[]);
        self.wait_idle(3_000).await;
        self.cmd(0x4E, &[0x00]);
        self.cmd(0x4F, &[0xC7, 0x00]);
        self.wait_idle(3_000).await;
        self.set_lut(&WF_FULL).await;
    }

    /// 全刷：0x22 0xC7 组合开关 → 0x20 点火 → 等忙（标称 ~2s，给 4 倍余量）
    async fn refresh_full(&mut self) {
        self.cmd(0x22, &[0xC7]);
        self.cmd(0x20, &[]);
        self.wait_idle(8_000).await;
    }

    /// 推帧：整帧写 0x24（0x26 第二帧面这节课不碰，留给第 6 课局刷）
    async fn push_frame(&mut self, fb: &[u8; 5_000]) {
        self.cmd(0x24, fb);
        self.refresh_full().await;
    }

    /// 局刷基准：0x24 与 0x26 写同一帧，再全刷（demo: DisplayPartBaseImage）
    async fn push_base(&mut self, fb: &[u8; 5_000]) {
        self.cmd(0x24, fb);
        self.cmd(0x26, fb);
        self.refresh_full().await;
    }

    /// RST 快翻：低 2ms → 高 2ms（demo 局刷序列开头；和 init 的 20/5/20 完全不同）
    async fn reset_fast(&mut self) {
        self.rst.set_low();
        Timer::after(Duration::from_millis(2)).await;
        self.rst.set_high();
        Timer::after(Duration::from_millis(2)).await;
    }

    /// 局刷一块矩形区域（第 5 课定的坐标系）。x、w 必须是 8 的倍数；y、h 不用
    async fn push_region(&mut self, fb: &[u8; 5_000], x: u16, y: u16, w: u16, h: u16) {
        // —— 换装：demo 每次局刷前都完整重做，照抄 ——
        self.reset_fast().await;
        self.set_lut(&WF_PARTIAL).await;
        self.cmd(
            0x37,
            &[0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00],
        );
        self.cmd(0x3C, &[0x80]);
        self.cmd(0x22, &[0xC0]);
        self.cmd(0x20, &[]);
        self.wait_idle(3_000).await;

        // —— 圈地：窗口。0x44 的 x 是字节地址（除以 8）；0x45 的 y 是行号，小端两字节
        let (x_end, y_end) = (x + w - 1, y + h - 1);
        self.cmd(0x44, &[(x >> 3) as u8, (x_end >> 3) as u8]);
        self.cmd(
            0x45,
            &[
                (y & 0xFF) as u8,
                (y >> 8) as u8,
                (y_end & 0xFF) as u8,
                (y_end >> 8) as u8,
            ],
        );

        // —— 落笔点：地址指针（demo 在设完指针后等了一次闲）——
        self.cmd(0x4E, &[(x >> 3) as u8]);
        self.cmd(0x4F, &[(y & 0xFF) as u8, (y >> 8) as u8]);
        self.wait_idle(3_000).await;

        // —— 只写窗口内的字节：一个 0x24 命令块，行与行之间 CS 不断开
        self.cs.set_low();
        self.dc.set_low();
        self.spi.write(&[0x24]).expect("SPI 写命令失败");
        self.dc.set_high();
        let bytes_per_row = (w >> 3) as usize;
        for row in y..=y_end {
            let first = (row as usize) * 25 + (x >> 3) as usize;
            self.spi
                .write(&fb[first..first + bytes_per_row])
                .expect("SPI 写数据失败");
        }
        self.cs.set_high();

        // —— 点火：局刷组合 0xCF ——
        self.cmd(0x22, &[0xCF]);
        self.cmd(0x20, &[]);
        self.wait_idle(1_500).await;
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

    // 帧缓冲住进 StaticCell（与第 5 课相同）
    static FB: StaticCell<FrameBuffer> = StaticCell::new();
    let fb = FB.init(FrameBuffer::new_white());

    // ===== 基准图：边框 + 起点(8,92) 的 16×16 方块 =====
    hline(fb, 0, 199, 0);
    hline(fb, 0, 199, 199);
    vline(fb, 0, 0, 199);
    vline(fb, 199, 0, 199);
    const SQ: u16 = 16; // 方块边长（x 方向 16 = 2 个字节，天然 8 对齐）
    const Y0: u16 = 92; // 方块所在行带：y 92..108
    let mut sx: u16 = 8; // 方块左边缘，永远是 8 的倍数
    for yy in Y0..Y0 + SQ {
        for xx in sx..sx + SQ {
            fb.set_pixel(xx, yy, false);
        }
    }

    // ===== 第 1 步：立基准（最后一次大扫除，~2s） =====
    esp_println::println!("立局刷基准……");
    epd.push_base(&fb.0).await;

    // ===== 第 2+3 步：局刷循环——方块向右滑 =====
    let mut partials: u32 = 0;
    loop {
        Timer::after(Duration::from_millis(600)).await;
        let old_x = sx;
        sx += SQ; // 右移一格
        if sx + SQ > 192 {
            sx = 8; // 到头回卷（右边留 7px，别压边框）
        }

        // 改画布：擦旧块（画白）、画新块（画黑）
        for yy in Y0..Y0 + SQ {
            for xx in old_x..old_x + SQ {
                fb.set_pixel(xx, yy, true);
            }
            for xx in sx..sx + SQ {
                fb.set_pixel(xx, yy, false);
            }
        }

        // 圈脏区：罩住「擦旧 + 画新」；回卷那步偷懒刷整条带
        if sx > old_x {
            epd.push_region(&fb.0, old_x, Y0, sx + SQ - old_x, SQ).await;
        } else {
            epd.push_region(&fb.0, 0, Y0, 200, SQ).await;
        }
        partials += 1;
        esp_println::println!("局刷 #{}, 方块 @x={}", partials, sx);
    }
}
