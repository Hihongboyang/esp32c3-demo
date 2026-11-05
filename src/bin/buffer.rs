/// 5000 字节画布。不变式：坐标永远以"代码约定方向"存储，1=白 0=黑，MSB 在前

/// ===== 实现 DrawTarget ======
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::OriginDimensions;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;

/// ===== 实现 DrawTarget ======
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

    /// 整帧刷白（L10：整帧重组的起点）。fill 直接写数组，无临时整帧
    pub(crate) fn clear_white(&mut self) {
        self.0.fill(0xFF);
    }
}

/// 画布当 DrawTarget 用：e-g 的图形对象逐像素交给我们，翻译成 set_pixel。
/// On = 白（写 1，第 4 课验证 / R7）；Off = 黑。
impl DrawTarget for FrameBuffer {
    type Color = BinaryColor;
    type Error = core::convert::Infallible; // 我们的绘制不会失败

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = embedded_graphics::Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels.into_iter() {
            // e-g 坐标是有符号 i32：负数或超出 200×200 的点直接裁掉
            if (0..200).contains(&coord.x) && (0..200).contains(&coord.y) {
                self.set_pixel(coord.x as u16, coord.y as u16, color == BinaryColor::On);
            }
        }
        Ok(())
    }
}

/// 告诉 e-g 这块画布有多大（原点默认 (0,0)）
impl OriginDimensions for FrameBuffer {
    fn size(&self) -> Size {
        Size::new(200, 200)
    }
}

