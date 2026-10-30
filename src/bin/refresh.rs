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
}

impl Refresher {
    pub(crate) fn new(epd: Epd) -> Self {
        Self {
            epd,
            shadow: FrameBuffer::new_white(),
            policy: Policy {
                max_partials: 10,
                full_ratio: 60,
            },
            partials: 0,
            base_ready: false,
            partial_cfg: false,
        }
    }

    /// 唯一渲染入口：自动 diff → 按规则选 跳过/全刷重建/局刷
    pub(crate) async fn present(&mut self, next: &FrameBuffer) {
        let (region, ratio) = match diff(next, &self.shadow) {
            None => {
                if LOG {
                    esp_println::println!("present: 两帧一致——跳过（零硬件动作）");
                }
                return;
            }
            Some(v) => v,
        };
        if !self.base_ready {
            // R2：缺基准，先补一次基准全刷（对上层透明，多花一次 2s）
            if LOG {
                esp_println::println!("present: 缺基准——全刷立基准（R2）");
            }
            self.epd.push_base(next.bytes()).await;
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
