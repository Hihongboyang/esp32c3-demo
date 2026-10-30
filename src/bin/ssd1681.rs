use crate::LOG;

use embassy_time::{Duration, Instant, Timer};
use esp_hal::Blocking;
use esp_hal::gpio::{Input, Output};
use esp_hal::spi::master::Spi;

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

pub(crate) struct Epd {
    pub(crate) spi: Spi<'static, Blocking>,
    pub(crate) cs: Output<'static>,
    pub(crate) dc: Output<'static>,
    pub(crate) rst: Output<'static>,
    pub(crate) busy: Input<'static>,
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

    /// 初始化：直译官方 demo HDirInit()
    pub(crate) async fn init(&mut self) {
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
        self.set_lut("WF_FULL", &WF_FULL).await;
    }

    /// 全刷：0x22 0xC7 组合开关 → 0x20 点火 → 等忙（标称 ~2s，给 4 倍余量）
    async fn refresh_full(&mut self) {
        self.cmd(0x22, &[0xC7]);
        self.cmd(0x20, &[]);
        self.wait_idle(8_000).await;
    }

    /// 推帧：整帧写 0x24（0x26 第二帧面这节课不碰，留给第 6 课局刷）
    #[allow(dead_code)]
    async fn push_frame(&mut self, fb: &[u8; 5_000]) {
        self.use_full_config().await; // 勘误 2：局刷会改动窗口与波形表，写整帧前先恢复
        self.cmd(0x24, fb);
        self.refresh_full().await;
    }

    /// 等 BUSY 拉低。返回 Some(高电平持续 ms)、None(超时)、Some(0)(刷新根本没开始——重要诊断信号)
    async fn wait_idle(&mut self, timeout_ms: u32) -> Option<u32> {
        let mut waited = 0u32;
        while self.busy.is_high() {
            if waited >= timeout_ms {
                esp_println::println!("[Epd] BUSY 超时 {}ms——查接线", timeout_ms);
                return None;
            }
            Timer::after(Duration::from_millis(10)).await;
            waited += 10;
        }
        Timer::after(Duration::from_millis(200)).await;
        Some(waited)
    }

    async fn set_lut(&mut self, name: &str, lut: &[u8; 159]) {
        let t = Instant::now();
        self.cmd(0x32, &lut[..153]);
        let busy = self.wait_idle(3_000).await;
        self.cmd(0x3F, &lut[153..154]);
        self.cmd(0x03, &lut[154..155]);
        self.cmd(0x04, &lut[155..158]);
        self.cmd(0x2C, &lut[158..159]);
        if LOG {
            esp_println::println!(
                "[Epd] LUT 装载 {} busy={:?}ms 耗时{}ms",
                name,
                busy,
                t.elapsed().as_millis()
            );
        }
    }

    /// 恢复全刷模式：整屏窗口、指针复位、边框跟随、装回全刷波形表。
    /// 局刷会改动这四样配置；任何整帧写入（push_base / push_frame）之前都必须先调用。
    /// （demo 的等价做法是每次局刷演示前重新 HDirInit 完整初始化）
    async fn use_full_config(&mut self) {
        self.cmd(0x44, &[0x00, 0x18]);
        self.cmd(0x45, &[0xC7, 0x00, 0x00, 0x00]);
        self.cmd(0x3C, &[0x01]);
        self.cmd(0x4E, &[0x00]);
        self.cmd(0x4F, &[0xC7, 0x00]);
        self.wait_idle(3_000).await;
        self.set_lut("WF_FULL", &WF_FULL).await;
    }

    /// 局刷基准：恢复全刷模式后，0x24 与 0x26 写同一帧，再全刷（demo: DisplayPartBaseImage）
    pub(crate) async fn push_base(&mut self, fb: &[u8; 5_000]) {
        self.use_full_config().await;
        self.cmd(0x24, fb);
        self.cmd(0x26, fb);
        self.refresh_full().await;
    }

    /// 进入局刷模式（结构照新版库的 Init_Partial，整个循环只做一次）。
    /// 关键差异（勘误 5）：0x37 第 6 字节 F[6] 是局刷模式的「RAM 乒乓」开关。
    /// demo 发 0x40 开乒乓、靠每步全量重写成立；窗口局刷必须关乒乓（0x00），
    /// 0x26 上一帧基准改由 push_region 在每次局刷后补写维护。
    pub(crate) async fn init_partial(&mut self) {
        let t = Instant::now();
        self.reset().await;
        self.wait_idle(3_000).await;
        self.cmd(0x01, &[0xC7, 0x00, 0x01]);
        self.cmd(0x11, &[0x01]);
        self.cmd(0x44, &[0x00, 0x18]);
        self.cmd(0x45, &[0xC7, 0x00, 0x00, 0x00]);
        self.set_lut("WF_PARTIAL", &WF_PARTIAL).await;
        self.cmd(
            0x37,
            //            └ 第 6 字节 = 0：RAM 乒乓关闭（demo 为 0x40 = 开启）
            &[0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        );
        self.cmd(0x3C, &[0x80]);
        self.cmd(0x22, &[0xC0]);
        self.cmd(0x20, &[]);
        let busy = self.wait_idle(3_000).await;
        if LOG {
            esp_println::println!(
                "[Epd] init_partial 完成: 乒乓=关, C0 装载 busy={:?}ms, 总耗时{}ms",
                busy,
                t.elapsed().as_millis()
            );
        }
    }

    /// 局刷一块矩形区域（第 5 课定的坐标系）。x、w 必须是 8 的倍数；y、h 不用。
    /// 配方（GxEPD2 在同款 SSD1681 上验证）：① 设窗口 → 写 0x24（新帧）
    /// ② 0x22 CF + 0x20 启动差分刷新 ③ 刷新完成后把同一窗口补写进 0x26——
    /// 乒乓关闭后基准由软件维护：不补写的话 0x26 停在两帧前，
    /// 旧方块位置的黑→白差异对不上，方块越积越多（勘误 5）。
    pub(crate) async fn push_region(&mut self, new: &[u8; 5_000], x: u16, y: u16, w: u16, h: u16) {
        let t = Instant::now();
        // —— 窗口与指针（勘误 1：镜像行号 199−y）——
        let (x_end, y_end) = (x + w - 1, y + h - 1);
        let ram_start = 199 - y; // fb 行 y 对应的 RAM 行
        let ram_end = 199 - y_end; // fb 行 y_end 对应的 RAM 行（ram_start ≥ ram_end）
        let bytes_per_row = (w >> 3) as usize;

        if LOG {
            // 首行头几个字节：确认发出去的数据（FF=白 00=黑）；窗口不足 4 字节也能打
            let head = &new[(y as usize) * 25 + (x >> 3) as usize..][..bytes_per_row.min(4)];
            esp_println::println!(
                "[局刷] 窗口 x[{},{}] y[{},{}] RAM行[{},{}] {}B/行 首行头={:02X?}",
                x,
                x_end,
                y,
                y_end,
                ram_start,
                ram_end,
                bytes_per_row,
                head
            );
        }

        // ① 写 0x24（新帧）
        self.set_window(x, x_end, ram_start, ram_end);
        self.write_region_plane(0x24, new, x, y, y_end, bytes_per_row);

        // ② 启动局刷：组合值 0xCF = 差分刷新（0x26 旧 → 0x24 新）
        self.cmd(0x22, &[0xCF]);
        self.cmd(0x20, &[]);
        let busy = self.wait_idle(3_000).await;
        if LOG {
            esp_println::println!("[局刷] 0xCF 启动刷新 busy={:?}ms (0=没刷!)", busy);
        }

        // ③ 刷新已把新帧搬上屏；补写 0x26 让基准跟上（GxEPD2: writeImageAgain）
        self.set_window(x, x_end, ram_start, ram_end);
        self.write_region_plane(0x26, new, x, y, y_end, bytes_per_row);
        if LOG {
            esp_println::println!(
                "[局刷] 0x26 基准补写完成, 本步总耗时{}ms",
                t.elapsed().as_millis()
            );
        }
    }

    /// 设置 RAM 窗口（0x44/0x45）与地址指针（0x4E/0x4F）。写每个帧面前各调一次
    fn set_window(&mut self, x: u16, x_end: u16, ram_start: u16, ram_end: u16) {
        self.cmd(0x44, &[(x >> 3) as u8, (x_end >> 3) as u8]);
        self.cmd(
            0x45,
            &[
                (ram_start & 0xFF) as u8,
                (ram_start >> 8) as u8,
                (ram_end & 0xFF) as u8,
                (ram_end >> 8) as u8,
            ],
        );
        self.cmd(0x4E, &[(x >> 3) as u8]);
        self.cmd(0x4F, &[(ram_start & 0xFF) as u8, (ram_start >> 8) as u8]);
    }

    /// 往指定帧面写窗口内的行数据：一个命令块，CS 不断开（0x24 新帧 / 0x26 基准维护）
    fn write_region_plane(
        &mut self,
        plane: u8,
        fb: &[u8; 5_000],
        x: u16,
        y: u16,
        y_end: u16,
        bytes_per_row: usize,
    ) {
        self.cs.set_low();
        self.dc.set_low();
        self.spi.write(&[plane]).expect("SPI 写命令失败");
        self.dc.set_high();
        for row in y..=y_end {
            let first = (row as usize) * 25 + (x >> 3) as usize;
            self.spi
                .write(&fb[first..first + bytes_per_row])
                .expect("SPI 写数据失败");
        }
        self.cs.set_high();
    }
}
