//! 鼠标键盘注入：Windows 走 SendInput，macOS 走 CGEvent，
//! 协议行为与 server.py InputController 兼容

#[cfg(windows)]
mod imp {
  use windows::Win32::Foundation::POINT;
  use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, VIRTUAL_KEY,
  };
  use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
  };

  fn special_key_vk(key: &str) -> Option<u16> {
    Some(match key.to_ascii_lowercase().as_str() {
      "enter" | "return" => 0x0D,
      "space" => 0x20,
      "tab" => 0x09,
      "backspace" => 0x08,
      "delete" => 0x2E,
      "esc" | "escape" => 0x1B,
      "shift" => 0x10,
      "ctrl" => 0x11,
      "alt" => 0x12,
      "alt_l" => 0xA4,
      "alt_r" => 0xA5,
      "cmd" | "win" => 0x5B,
      "up" => 0x26,
      "down" => 0x28,
      "left" => 0x25,
      "right" => 0x27,
      "home" => 0x24,
      "end" => 0x23,
      "page_up" => 0x21,
      "page_down" => 0x22,
      "caps_lock" => 0x14,
      "num_lock" => 0x90,
      "insert" => 0x2D,
      "print_screen" => 0x2C,
      "scroll_lock" => 0x91,
      "pause" => 0x13,
      "f1" => 0x70,
      "f2" => 0x71,
      "f3" => 0x72,
      "f4" => 0x73,
      "f5" => 0x74,
      "f6" => 0x75,
      "f7" => 0x76,
      "f8" => 0x77,
      "f9" => 0x78,
      "f10" => 0x79,
      "f11" => 0x7A,
      "f12" => 0x7B,
      _ => return None,
    })
  }

  /// 单字符：小写字母/数字走 VK（可配合修饰键）；其余走 UNICODE 扫描码
  fn char_code(c: char) -> (u16, u16, bool) {
    if c.is_ascii_lowercase() {
      (0x41 + (c as u16 - 'a' as u16), 0, false)
    } else if c.is_ascii_digit() {
      (0x30 + (c as u16 - '0' as u16), 0, false)
    } else {
      (0, c as u16, true)
    }
  }

  fn send_key(vk: u16, scan: u16, unicode: bool, pressed: bool) {
    let mut input = INPUT {
      r#type: INPUT_KEYBOARD,
      ..Default::default()
    };
    let mut flags = 0u32;
    if unicode {
      flags |= KEYEVENTF_UNICODE.0;
    }
    if !pressed {
      flags |= KEYEVENTF_KEYUP.0;
    }
    input.Anonymous.ki = KEYBDINPUT {
      wVk: VIRTUAL_KEY(vk),
      wScan: scan,
      dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::KEYBD_EVENT_FLAGS(flags),
      time: 0,
      dwExtraInfo: 0,
    };
    unsafe {
      SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
  }

  fn send_mouse(dx: i32, dy: i32, mouse_data: u32, flags: u32) {
    let mut input = INPUT {
      r#type: INPUT_MOUSE,
      ..Default::default()
    };
    input.Anonymous.mi = MOUSEINPUT {
      dx,
      dy,
      mouseData: mouse_data,
      dwFlags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS(flags),
      time: 0,
      dwExtraInfo: 0,
    };
    unsafe {
      SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
  }

  pub fn mouse_move(x: i32, y: i32) {
    let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if vw <= 0 || vh <= 0 {
      return;
    }
    let norm_x = ((x - vx) * 65535 / vw).clamp(0, 65535);
    let norm_y = ((y - vy) * 65535 / vh).clamp(0, 65535);
    send_mouse(
      norm_x,
      norm_y,
      0,
      MOUSEEVENTF_MOVE.0 | MOUSEEVENTF_ABSOLUTE.0 | MOUSEEVENTF_VIRTUALDESK.0,
    );
  }

  pub fn mouse_move_relative(dx: i32, dy: i32) {
    if dx == 0 && dy == 0 {
      return;
    }
    let mut pt = POINT::default();
    if unsafe { GetCursorPos(&mut pt) }.is_ok() {
      mouse_move(pt.x + dx, pt.y + dy);
    }
  }

  pub fn mouse_button(button: &str, pressed: bool) {
    let flags = match (button.to_ascii_lowercase().as_str(), pressed) {
      ("right", true) => MOUSEEVENTF_RIGHTDOWN.0,
      ("right", false) => MOUSEEVENTF_RIGHTUP.0,
      ("middle", true) => MOUSEEVENTF_MIDDLEDOWN.0,
      ("middle", false) => MOUSEEVENTF_MIDDLEUP.0,
      ("left", true) => MOUSEEVENTF_LEFTDOWN.0,
      _ => MOUSEEVENTF_LEFTUP.0,
    };
    send_mouse(0, 0, 0, flags);
  }

  pub fn mouse_scroll(dy: i32) {
    if dy == 0 {
      return;
    }
    let wheel = (dy * 120) as u32;
    send_mouse(0, 0, wheel, MOUSEEVENTF_WHEEL.0);
  }

  pub fn key_event(key: &str, pressed: bool) {
    if let Some(vk) = special_key_vk(key) {
      send_key(vk, 0, false, pressed);
      return;
    }
    let mut chars = key.chars();
    if let Some(c) = chars.next() {
      if chars.next().is_none() {
        let (vk, scan, unicode) = char_code(c);
        if vk != 0 || unicode {
          send_key(vk, scan, unicode, pressed);
        }
      }
    }
  }

  pub fn type_text(text: &str) {
    for c in text.chars() {
      send_key(0, c as u16, true, true);
      send_key(0, c as u16, true, false);
    }
  }

  pub fn cursor_pos() -> (i32, i32) {
    let mut pt = POINT::default();
    if unsafe { GetCursorPos(&mut pt) }.is_ok() {
      (pt.x, pt.y)
    } else {
      (0, 0)
    }
  }
}

#[cfg(target_os = "macos")]
mod imp {
  use std::sync::Mutex;

  use core_graphics::display::CGDisplay;
  use core_graphics::event::{
    CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, ScrollEventUnit,
  };
  use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
  use core_graphics::geometry::CGPoint;

  /// 当前按下的鼠标键。macOS 要拖动必须发 *MouseDragged 事件，
  /// 而协议里拖动只是 MouseDown + MouseMove，所以这里记住按键状态。
  static PRESSED: Mutex<Option<CGMouseButton>> = Mutex::new(None);

  /// 截图是像素分辨率（Retina 通常 2x），CGEvent 坐标是逻辑点，
  /// 二者比例即主屏的 backing scale。
  fn display_scale() -> f64 {
    let d = CGDisplay::main();
    let px = d.pixels_wide() as f64;
    let pt = d.bounds().size.width;
    if pt > 0.0 {
      px / pt
    } else {
      1.0
    }
  }

  fn source() -> Option<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()
  }

  /// 当前鼠标位置（逻辑点）。CGEventCreate 产出的空事件带当前光标位置。
  fn current_point() -> CGPoint {
    source()
      .and_then(|s| CGEvent::new(s).ok())
      .map(|e| e.location())
      .unwrap_or(CGPoint::new(0.0, 0.0))
  }

  fn pressed_button() -> Option<CGMouseButton> {
    PRESSED.lock().map(|g| *g).unwrap_or(None)
  }

  fn post_mouse(event_type: CGEventType, point: CGPoint, button: CGMouseButton) {
    if let Some(src) = source() {
      if let Ok(ev) = CGEvent::new_mouse_event(src, event_type, point, button) {
        ev.post(CGEventTapLocation::HID);
      }
    }
  }

  fn post_key(keycode: u16, pressed: bool) {
    if let Some(src) = source() {
      if let Ok(ev) = CGEvent::new_keyboard_event(src, keycode, pressed) {
        ev.post(CGEventTapLocation::HID);
      }
    }
  }

  fn post_unicode(text: &str, pressed: bool) {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    if let Some(src) = source() {
      if let Ok(ev) = CGEvent::new_keyboard_event(src, 0, pressed) {
        ev.set_string_from_utf16_unchecked(&utf16);
        ev.post(CGEventTapLocation::HID);
      }
    }
  }

  fn drag_event(button: CGMouseButton) -> CGEventType {
    match button {
      CGMouseButton::Right => CGEventType::RightMouseDragged,
      CGMouseButton::Center => CGEventType::OtherMouseDragged,
      CGMouseButton::Left => CGEventType::LeftMouseDragged,
    }
  }

  /// 移动光标：按住键时发拖动事件，否则发普通移动
  fn move_to(point: CGPoint) {
    match pressed_button() {
      Some(btn) => post_mouse(drag_event(btn), point, btn),
      None => post_mouse(CGEventType::MouseMoved, point, CGMouseButton::Left),
    }
  }

  /// 命名键 → macOS 虚拟键码（ANSI 布局）
  fn special_keycode(key: &str) -> Option<u16> {
    Some(match key.to_ascii_lowercase().as_str() {
      "enter" | "return" => 36,
      "tab" => 48,
      "space" => 49,
      "backspace" => 51,
      "delete" => 117,
      "esc" | "escape" => 53,
      "shift" => 56,
      "ctrl" | "control" => 59,
      "alt" | "option" | "alt_l" => 58,
      "alt_r" => 61,
      "cmd" | "win" | "command" => 55,
      "up" => 126,
      "down" => 125,
      "left" => 123,
      "right" => 124,
      "home" => 115,
      "end" => 119,
      "page_up" => 116,
      "page_down" => 121,
      "caps_lock" => 57,
      "num_lock" => 71,
      "insert" => 114,
      "f1" => 122,
      "f2" => 120,
      "f3" => 99,
      "f4" => 118,
      "f5" => 96,
      "f6" => 97,
      "f7" => 98,
      "f8" => 100,
      "f9" => 101,
      "f10" => 109,
      "f11" => 103,
      "f12" => 111,
      _ => return None,
    })
  }

  /// 小写字母/数字 → macOS 虚拟键码；其余字符走 Unicode 注入
  fn char_keycode(c: char) -> Option<u16> {
    Some(match c {
      'a' => 0,
      's' => 1,
      'd' => 2,
      'f' => 3,
      'h' => 4,
      'g' => 5,
      'z' => 6,
      'x' => 7,
      'c' => 8,
      'v' => 9,
      'b' => 11,
      'q' => 12,
      'w' => 13,
      'e' => 14,
      'r' => 15,
      'y' => 16,
      't' => 17,
      '1' => 18,
      '2' => 19,
      '3' => 20,
      '4' => 21,
      '6' => 22,
      '5' => 23,
      '9' => 25,
      '7' => 26,
      '8' => 28,
      '0' => 29,
      'o' => 31,
      'u' => 32,
      'i' => 34,
      'p' => 35,
      'l' => 37,
      'j' => 38,
      'k' => 40,
      'n' => 45,
      'm' => 46,
      _ => return None,
    })
  }

  pub fn mouse_move(x: i32, y: i32) {
    let s = display_scale();
    move_to(CGPoint::new(x as f64 / s, y as f64 / s));
  }

  pub fn mouse_move_relative(dx: i32, dy: i32) {
    if dx == 0 && dy == 0 {
      return;
    }
    let s = display_scale();
    let cur = current_point();
    move_to(CGPoint::new(cur.x + dx as f64 / s, cur.y + dy as f64 / s));
  }

  pub fn mouse_button(button: &str, pressed: bool) {
    let btn = match button.to_ascii_lowercase().as_str() {
      "right" => CGMouseButton::Right,
      "middle" => CGMouseButton::Center,
      _ => CGMouseButton::Left,
    };
    if let Ok(mut g) = PRESSED.lock() {
      *g = if pressed { Some(btn) } else { None };
    }
    let event_type = match (btn, pressed) {
      (CGMouseButton::Right, true) => CGEventType::RightMouseDown,
      (CGMouseButton::Right, false) => CGEventType::RightMouseUp,
      (CGMouseButton::Center, true) => CGEventType::OtherMouseDown,
      (CGMouseButton::Center, false) => CGEventType::OtherMouseUp,
      (_, true) => CGEventType::LeftMouseDown,
      (_, false) => CGEventType::LeftMouseUp,
    };
    post_mouse(event_type, current_point(), btn);
  }

  pub fn mouse_scroll(dy: i32) {
    if dy == 0 {
      return;
    }
    if let Some(src) = source() {
      if let Ok(ev) = CGEvent::new_scroll_event(src, ScrollEventUnit::LINE, 1, dy, 0, 0) {
        ev.post(CGEventTapLocation::HID);
      }
    }
  }

  pub fn key_event(key: &str, pressed: bool) {
    if let Some(kc) = special_keycode(key) {
      post_key(kc, pressed);
      return;
    }
    let mut chars = key.chars();
    if let Some(c) = chars.next() {
      if chars.next().is_none() {
        match char_keycode(c) {
          Some(kc) => post_key(kc, pressed),
          None => post_unicode(&c.to_string(), pressed),
        }
      }
    }
  }

  pub fn type_text(text: &str) {
    for c in text.chars() {
      let s = c.to_string();
      post_unicode(&s, true);
      post_unicode(&s, false);
    }
  }

  pub fn cursor_pos() -> (i32, i32) {
    let s = display_scale();
    let p = current_point();
    ((p.x * s).round() as i32, (p.y * s).round() as i32)
  }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
  pub fn mouse_move(_x: i32, _y: i32) {}
  pub fn mouse_move_relative(_dx: i32, _dy: i32) {}
  pub fn mouse_button(_button: &str, _pressed: bool) {}
  pub fn mouse_scroll(_dy: i32) {}
  pub fn key_event(_key: &str, _pressed: bool) {}
  pub fn type_text(_text: &str) {}
  pub fn cursor_pos() -> (i32, i32) {
    (0, 0)
  }
}

pub use imp::{
  cursor_pos, key_event, mouse_button, mouse_move, mouse_move_relative, mouse_scroll, type_text,
};
