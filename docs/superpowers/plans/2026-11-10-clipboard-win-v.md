# Win+V clipboard takeover

User-approved behavior: optional persisted switch, Win+V opens Daozai instead of
the Windows history panel, Ctrl+Alt+V stays available, disabling or exiting restores
the system route. Existing configurations default to off; enable for this user
after building and checking the settings UI.

Implementation:
1. Pure key-state reducer with physical V press/release ownership, repeat suppression,
   exact Win-only modifiers, injected-event pass-through, and disable cleanup.
2. Reuse the existing input hook. Consume only the owned V events, leave Win events
   untouched, and mask the otherwise isolated Win press using the dummy-key pair
   described by Microsoft PowerToys. Keep callbacks short and request a UI repaint.
3. Dispatch the popup after physical modifiers are released. Keep backup hotkey
   registration independent; surface hook/masking failures and revert to system routing.
4. Persist the switch, expose it in clipboard settings, update shortcut hints.
5. Run reducer, hook routing, settings persistence, layout and full regression tests;
   build release and verify settings/backup shortcut in the desktop app. The desktop
   automation skill forbids sending Win-key shortcuts, so do not simulate Win+V on
   the live desktop or claim automated physical-key acceptance.

Reference: Microsoft PowerToys keyboard manager developer documentation, section
"Dummy key events"; Microsoft Learn LowLevelKeyboardProc and RegisterHotKey.

Acceptance (2026-11-10): after enabling the release build, the user manually pressed
and released Win+V and confirmed that only the Daozai clipboard appeared. This is
user-reported desktop verification; physical Win+V restoration after disabling or
exiting has not been separately exercised on the desktop.
