use skia_safe::Color;

pub const INK_0: u32 = 0xff06_090f;
pub const INK_1: u32 = 0xff0b_0f18;
pub const INK_2: u32 = 0xff11_1726;
pub const PANEL: u32 = 0xff0f_1420;
pub const PANEL_RAISED: u32 = 0xff15_1b2b;
pub const HAIRLINE: u32 = 0xff2a_3346;
pub const HAIRLINE_DIM: u32 = 0x332a_3346;
pub const TEXT: u32 = 0xffec_e6d6;
pub const TEXT_DIM: u32 = 0xff97_a0b4;
pub const TEXT_FAINT: u32 = 0xff5a_6478;
pub const SIGNAL: u32 = 0xffe8_a44c;
pub const SIGNAL_HOT: u32 = 0xffff_6b4a;
pub const SIGNAL_HOT_DEEP: u32 = 0xffc2_3a24;
pub const DATA: u32 = 0xff4c_c9d6;
pub const PLUM: u32 = 0xff7c_5cff;
pub const BRIDGE: u32 = 0xff39_445a;
pub const BRIDGE_HIGHLIGHT: u32 = 0xff8b_97b5;
pub const GRID: u32 = 0x0bff_ffff;
pub const GRID_MAJOR: u32 = 0x12ff_ffff;

pub const DISPLAY_FONT: &str = "Manrope";
pub const MONO_FONT: &str = "JetBrainsMono";

pub fn color(argb: u32) -> Color {
    Color::from_argb(
        ((argb >> 24) & 0xff) as u8,
        ((argb >> 16) & 0xff) as u8,
        ((argb >> 8) & 0xff) as u8,
        (argb & 0xff) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argb_constants_retain_flutter_channel_order() {
        let signal = color(SIGNAL);
        assert_eq!(
            (signal.a(), signal.r(), signal.g(), signal.b()),
            (255, 232, 164, 76)
        );
        let grid = color(GRID);
        assert_eq!(
            (grid.a(), grid.r(), grid.g(), grid.b()),
            (11, 255, 255, 255)
        );
    }
}
