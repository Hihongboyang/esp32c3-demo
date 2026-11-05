use crate::LOG;
use crate::buffer::FrameBuffer;
use crate::ssd1681::Epd;

/// 策略参数（DESIGN §2.6）：参数属于你的屏，不是天上的
struct Policy {
    max_partials: u32, // R9：局刷链上限，达到后转全刷重建基准
    full_ratio: u8,    // 变更占比 ≥ 此值（%）直接全刷
}

/// 刷新引擎：DESIGN §4.3 门面的雏形——业务只跟它说话
pub(crate) struct Refresher {
    epd: Epd,
    pub(crate) shadow: FrameBuffer, // R8：屏上确认过的帧，唯一真相（diff 的比较对象）
    policy: Policy,
    partials: u32,     // 自上次全刷以来的局刷次数
    base_ready: bool,  // 局刷基准是否已立（R2）
    partial_cfg: bool, // 芯片是否处于局刷配置——push_base 会把它切回全刷（第 6 课勘误 5）
    fulls: u32,        // 累计全刷次数
    force_full: bool,  // R1：屏深睡唤醒后置位——下一次 present 无条件全刷重建，含绕过"跳过"
}

impl Refresher {
    pub(crate) fn new(epd: Epd) -> Self {
        Self {
            epd,
            shadow: FrameBuffer::new_white(),
            fulls: 0,
            policy: Policy {
                max_partials: 10,
                full_ratio: 60,
            },
            partials: 0,
            base_ready: false,
            partial_cfg: false,
            force_full: false,
        }
    }

    /// 唯一渲染入口：自动 diff → 按规则选 跳过/全刷重建/局刷
    pub(crate) async fn present(&mut self, next: &FrameBuffer) {
        let (region, ratio) = match diff(next, &self.shadow) {
            None if !self.force_full => {
                if LOG {
                    esp_println::println!("present: 两帧一致——跳过（零硬件动作）");
                }
                return;
            }
            // force_full 时 diff = None 也要往下走: R1 唤醒后即使两帧一致
            // 0x26 基准也不可信, 必须全刷重键( Region 占位, 全刷分支不用它 )
            None => (
                Region {
                    x: 0,
                    y: 0,
                    w: 0,
                    h: 0,
                },
                0,
            ),
            Some(v) => v,
        };
        if self.force_full || !self.base_ready {
            // R1/R2：全刷立基准。R1=屏深睡唤醒后 RAM 不保证——哪怕画面看起来还在
            // （双稳态保持），0x26 也不能再用；R2=开机缺基准，对上层透明
            if LOG {
                let why = if self.force_full {
                    "R1 唤醒重建 "
                } else {
                    "缺基准(R2)"
                };
                esp_println::println!("present: {why}——全刷立基准");
            }
            self.epd.push_base(next.bytes()).await;
            self.force_full = false;
            self.fulls += 1;
            self.base_ready = true;
            self.partials = 0;
            self.partial_cfg = false; // 全刷把 LUT/边框切回了全刷配置
        } else if ratio >= self.policy.full_ratio || self.partials >= self.policy.max_partials {
            // R9：变更太大局刷无收益，或局刷次数达到上限——全刷重建基准，同时清除残影
            if LOG {
                esp_println::println!(
                    "present: 全刷重建基准（占比={}%, 局刷次数={}/{}）",
                    ratio,
                    self.partials,
                    self.policy.max_partials
                );
            }
            self.epd.push_base(next.bytes()).await;
            self.fulls += 1;
            self.partials = 0;
            self.partial_cfg = false;
        } else {
            if !self.partial_cfg {
                // 上次全刷之后的第一次局刷：重新装载局刷配置（一次性，之后每步只执行）
                if LOG {
                    esp_println::println!("present: 重进局刷模式（init_partial）");
                }
                self.epd.init_partial().await;
                self.partial_cfg = true;
            }
            if LOG {
                esp_println::println!(
                    "present: 局刷 {}×{} @({},{}), ratio={}%",
                    region.w,
                    region.h,
                    region.x,
                    region.y,
                    ratio
                );
            }
            // v3 签名：只传新帧；0x26 由 push_region 在局刷后补写维护
            self.epd
                .push_region(next.bytes(), region.x, region.y, region.w, region.h)
                .await;
            self.partials += 1;
        }
        // R8：刷新成功后更新影子——屏上是什么，影子就是什么
        self.shadow.copy_from(next);
    }

    pub(crate) fn stats(&self) -> (u32, u32) {
        (self.fulls, self.partials)
    }
    
    /// 屏进入深睡（画面保持，功耗降到微安级）。R1：唤醒后 0x26 基准不可信——
    /// 置 force_full，下一次 present 无条件全刷重建（即使画面没变也不许"跳过"）
    pub(crate) async fn hibernate_screen(&mut self) {
        self.epd.sleep().await;
        self.force_full = true;
    }

    /// 屏唤醒：RST 复位脉冲 + 完整初始化（深睡后 RAM 不保证，init 全部重来）
    pub(crate) async fn wake_screen(&mut self) {
        self.epd.init().await;
    }
}

/// 脏区矩形：面板原生坐标系（第 5 课的约定），x/w 天然 8 对齐
pub(crate) struct Region {
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) w: u16,
    pub(crate) h: u16,
}

/// 帧差分（DESIGN L2 / R5/R8）：字节级比较
/// None = 两帧一致；Some((脏区, 变更像素占比 0-100))
pub(crate) fn diff(next: &FrameBuffer, shadow: &FrameBuffer) -> Option<(Region, u8)> {
    let mut min_y = 200u16;
    let mut max_y = 0u16;
    let mut min_xb = 25usize; // 字节坐标（含）
    let mut max_xb = 0usize;
    let mut changed = 0u32;
    for y in 0..200u16 {
        let base = (y as usize) * 25;
        let mut row_min = 25usize;
        let mut row_max = 0usize;
        for xb in 0..25usize {
            let d = next.bytes()[base + xb] ^ shadow.bytes()[base + xb];
            if d != 0 {
                changed += d.count_ones();
                if xb < row_min {
                    row_min = xb;
                }
                if xb > row_max {
                    row_max = xb;
                }
            }
        }
        if row_min <= row_max {
            // 本行有差异：并入全局包围盒
            if y < min_y {
                min_y = y;
            }
            if y > max_y {
                max_y = y;
            }
            if row_min < min_xb {
                min_xb = row_min;
            }
            if row_max > max_xb {
                max_xb = row_max;
            }
        }
    }
    if changed == 0 {
        return None;
    }
    Some((
        Region {
            x: (min_xb as u16) * 8,
            y: min_y,
            w: ((max_xb - min_xb + 1) as u16) * 8,
            h: max_y - min_y + 1,
        },
        (changed * 100 / 40_000) as u8,
    ))
}
