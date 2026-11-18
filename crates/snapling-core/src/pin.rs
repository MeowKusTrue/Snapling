//! Shared pin opacity: keep a visible minimum so it can always be recovered.
pub fn opacity_step(alpha: u8, up: bool) -> u8 {
    (i16::from(alpha) + if up { 20 } else { -20 }).clamp(40, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opacity_stays_visible_and_can_return_to_full() {
        assert_eq!(opacity_step(255, true), 255);
        assert_eq!(opacity_step(255, false), 235);
        assert_eq!(opacity_step(40, false), 40);
        assert_eq!(opacity_step(40, true), 60);
    }
}
