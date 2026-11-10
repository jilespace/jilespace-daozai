//! State for the optional Win+V takeover, independent of Windows input injection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Decision {
    pub suppress: bool,
    pub mask_start_menu: bool,
}

#[derive(Default)]
pub(crate) struct WinV {
    enabled: bool,
    v_down: bool,
    captured: bool,
    pending: bool,
}

impl WinV {
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled { self.pending = false; }
    }
    pub fn key(&mut self, vk: u32, down: bool, injected: bool, win: bool, other_modifiers: bool) -> Decision {
        if injected || vk != 0x56 { return Decision::default(); }
        let was_down = std::mem::replace(&mut self.v_down, down);
        if self.captured {
            if !down { self.captured = false; }
            return Decision { suppress: true, mask_start_menu: false };
        }
        if down && !was_down && self.enabled && win && !other_modifiers {
            self.captured = true;
            self.pending = true;
            return Decision { suppress: true, mask_start_menu: true };
        }
        Decision::default()
    }
    pub fn take(&mut self, keys_released: bool) -> bool {
        keys_released && !self.v_down && std::mem::take(&mut self.pending)
    }
    pub fn pending(&self) -> bool { self.pending }
    pub fn enabled(&self) -> bool { self.enabled }
    pub fn abort_capture(&mut self) {
        self.set_enabled(false);
        self.captured = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const V: u32 = 0x56;
    fn enabled() -> WinV { let mut state = WinV::default(); state.set_enabled(true); state }

    #[test]
    fn win_v_takes_over_one_physical_chord_and_waits_for_release() {
        let mut state = enabled();
        assert_eq!(state.key(V, true, false, true, false), Decision { suppress: true, mask_start_menu: true });
        for _ in 0..4 {
            assert_eq!(state.key(V, true, false, true, false), Decision { suppress: true, mask_start_menu: false });
        }
        assert!(!state.take(false));
        assert!(state.key(V, false, false, false, false).suppress, "own the matching release even if Win released first");
        assert!(state.take(true));
        assert!(!state.take(true), "consume the request once");
        assert!(state.key(V, true, false, true, false).mask_start_menu, "next physical press can reopen");
    }

    #[test]
    fn win_v_leaves_other_shortcuts_typing_and_injected_events_untouched() {
        for (vk, injected, win, other) in [(V,false,false,false), (V,false,true,true),
            (V,true,true,false), (0x43,false,true,false), (0x20,false,true,false),
            (0x5b,false,true,false), (0x5c,false,true,false)] {
            let mut state = enabled();
            assert_eq!(state.key(vk,true,injected,win,other), Decision::default());
            assert_eq!(state.key(vk,false,injected,win,other), Decision::default());
            assert!(!state.take(true));
        }
    }

    #[test]
    fn win_v_disable_finishes_owned_key_release_without_reopening() {
        let mut state = enabled();
        assert!(state.key(V,true,false,true,false).suppress);
        state.set_enabled(false);
        assert!(state.key(V,true,false,false,false).suppress);
        assert!(state.key(V,false,false,false,false).suppress);
        assert!(!state.take(true));
        assert_eq!(state.key(V,true,false,true,false), Decision::default());
    }

    #[test]
    fn win_v_does_not_capture_a_v_that_was_already_held() {
        let mut state = WinV::default();
        assert_eq!(state.key(V,true,false,false,false), Decision::default());
        state.set_enabled(true);
        assert_eq!(state.key(V,true,false,true,false), Decision::default());
        state.key(V,false,false,true,false);
        assert!(state.key(V,true,false,true,false).suppress);
    }

    #[test]
    fn win_v_injected_release_cannot_end_a_physical_capture() {
        let mut state = enabled();
        state.key(V,true,false,true,false);
        assert_eq!(state.key(V,false,true,false,false), Decision::default());
        assert!(state.key(V,true,false,true,false).suppress);
        assert!(!state.key(V,true,false,true,false).mask_start_menu);
        state.key(V,false,false,true,false);
        assert!(state.take(true));
    }

    #[test]
    fn win_v_mask_failure_returns_the_chord_to_windows() {
        let mut state = enabled();
        assert!(state.key(V,true,false,true,false).mask_start_menu);
        state.abort_capture();
        assert!(!state.enabled());
        assert!(!state.take(true));
        assert_eq!(state.key(V,false,false,false,false), Decision::default());
        assert_eq!(state.key(V,true,false,true,false), Decision::default());
    }
}
