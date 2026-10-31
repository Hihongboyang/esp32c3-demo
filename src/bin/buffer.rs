/// 5000 字节画布。不变式：坐标永远以"代码约定方向"存储，1=白 0=黑，MSB 在前
pub(crate) struct FrameBuffer([u8; 5_000]);

impl FrameBuffer {
    pub(crate) fn new_white() -> Self {
        Self([0xFF; 5_000])
    }

    /// x、y ∈ 0..199。white=true 画白点，false 画黑点
    pub(crate) fn set_pixel(&mut self, x: u16, y: u16, white: bool) {
        let byte = (y as usize) * 25 + (x >> 3) as usize;
        let bit = 7 - (x & 7); // MSB 在前：x=0 用 bit7
        if white {
            self.0[byte] |= 1 << bit; // 置 1 = 白
        } else {
            self.0[byte] &= !(1 << bit); // 清 0 = 黑
        }
    }

    /// 返回内部值
    pub(crate) fn bytes(&self) -> &[u8; 5_000] {
        &self.0
    }

    /// 整帧拷贝：把 other 的内容复制进自己，覆盖全部 5000 字节（R8：影子帧同步用）
    pub(crate) fn copy_from(&mut self, other: &FrameBuffer) {
        self.0.copy_from_slice(&other.0);
    }
}

/// 水平线（闭区间 [x0, x1]）
pub(crate) fn hline(fb: &mut FrameBuffer, x0: u16, x1: u16, y: u16) {
    for x in x0..=x1 {
        fb.set_pixel(x, y, false);
    }
}

/// 垂直线（闭区间 [y0, y1]）
pub(crate) fn vline(fb: &mut FrameBuffer, x: u16, y0: u16, y1: u16) {
    for y in y0..=y1 {
        fb.set_pixel(x, y, false);
    }
}

/// Bresenham 直线：纯整数运算画任意方向线段（1965 年的算法，嵌入式至今在用）
#[allow(dead_code)]
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

/// 整屏反色：制造 100% 变更，演示「大变更直接转全刷」
pub(crate) fn invert(fb: &mut FrameBuffer) {
    for b in fb.0.iter_mut() {
        *b ^= 0xFF;
    }
}
