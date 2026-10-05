//! 屏幕捕获：Windows 走 GDI BitBlt 全虚拟屏幕 + 系统光标合成，
//! macOS 走 CGDisplayCreateImage（主屏像素分辨率）
//! 行为对齐 server.py 的 mss/dxcam + GDI 光标覆盖

#[cfg(windows)]
mod imp {
  use std::mem::size_of;

  use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    GetObjectW, ReleaseDC, SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HDC, HGDIOBJ, SRCCOPY,
  };
  use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorInfo, GetIconInfo, GetSystemMetrics, CURSORINFO, CURSOR_SHOWING, ICONINFO,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
  };

  /// 一帧 BGRA 像素（top-down 行序，含合成后的光标）
  pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
  }

  fn bgra_of_bitmap(
    hdc: HDC,
    hbmp: windows::Win32::Graphics::Gdi::HBITMAP,
    w: u32,
    h: u32,
  ) -> Option<Vec<u8>> {
    let mut bmi = BITMAPINFO {
      bmiHeader: BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w as i32,
        biHeight: -(h as i32), // top-down
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
      },
      bmiColors: [Default::default()],
    };
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let got = unsafe {
      GetDIBits(
        hdc,
        hbmp,
        0,
        h,
        Some(buf.as_mut_ptr() as *mut _),
        &mut bmi,
        DIB_RGB_COLORS,
      )
    };
    if got == 0 {
      return None;
    }
    Some(buf)
  }

  /// 合成光标：彩色位图直接覆盖；单色 mask 转黑/白像素
  #[allow(clippy::too_many_arguments)]
  fn overlay_cursor(
    frame: &mut [u8],
    w: u32,
    h: u32,
    x: i32,
    y: i32,
    cursor_bgra: &[u8],
    cw: u32,
    ch: u32,
  ) {
    for cy in 0..ch {
      for cx in 0..cw {
        let sx = x + cx as i32;
        let sy = y + cy as i32;
        if sx < 0 || sy < 0 || sx >= w as i32 || sy >= h as i32 {
          continue;
        }
        let si = ((cy * cw + cx) * 4) as usize;
        let a = cursor_bgra[si + 3];
        if a < 32 {
          continue;
        }
        let di = ((sy as u32 * w + sx as u32) * 4) as usize;
        if a >= 250 {
          frame[di..di + 4].copy_from_slice(&cursor_bgra[si..si + 4]);
        } else {
          let da = 255 - a;
          frame[di] =
            ((cursor_bgra[si + 2] as u32 * a as u32 + frame[di] as u32 * da as u32) / 255) as u8;
          frame[di + 1] = ((cursor_bgra[si + 1] as u32 * a as u32
            + frame[di + 1] as u32 * da as u32)
            / 255) as u8;
          frame[di + 2] =
            ((cursor_bgra[si] as u32 * a as u32 + frame[di + 2] as u32 * da as u32) / 255) as u8;
        }
      }
    }
  }

  /// 捕获整个虚拟屏幕（含多显示器），合成当前光标
  pub fn capture_screen() -> Result<Frame, String> {
    let vx = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let vy = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let vw = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let vh = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if vw <= 0 || vh <= 0 {
      return Err("屏幕尺寸非法".to_string());
    }
    let w = vw as u32;
    let h = vh as u32;

    let screen_hdc = unsafe { GetDC(None) };
    if screen_hdc.is_invalid() {
      return Err("获取屏幕 DC 失败".to_string());
    }
    let mem_hdc = unsafe { CreateCompatibleDC(Some(screen_hdc)) };
    let hbmp = unsafe { CreateCompatibleBitmap(screen_hdc, vw, vh) };
    if mem_hdc.is_invalid() || hbmp.is_invalid() {
      unsafe { ReleaseDC(None, screen_hdc) };
      return Err("创建内存 DC/位图失败".to_string());
    }
    unsafe {
      SelectObject(mem_hdc, HGDIOBJ(hbmp.0));
      if !BitBlt(mem_hdc, 0, 0, vw, vh, Some(screen_hdc), vx, vy, SRCCOPY).is_ok() {
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        let _ = DeleteDC(mem_hdc);
        let _ = ReleaseDC(None, screen_hdc);
        return Err("BitBlt 截屏失败".to_string());
      }
    }

    let mut frame =
      bgra_of_bitmap(mem_hdc, hbmp, w, h).unwrap_or_else(|| vec![0u8; (w * h * 4) as usize]);

    // ---- 光标合成 ----
    let mut ci = CURSORINFO {
      cbSize: size_of::<CURSORINFO>() as u32,
      ..Default::default()
    };
    if unsafe { GetCursorInfo(&mut ci) }.is_ok()
      && ci.flags == CURSOR_SHOWING
      && !ci.hCursor.0.is_null()
    {
      let mut ii = ICONINFO::default();
      let hicon = windows::Win32::UI::WindowsAndMessaging::HICON(ci.hCursor.0);
      if unsafe { GetIconInfo(hicon, &mut ii) }.is_ok() {
        let cw;
        let ch;
        let cursor_pixels;
        if !ii.hbmColor.is_invalid() {
          if let Some(bm) = bitmap_dims(ii.hbmColor) {
            cw = bm.0;
            ch = bm.1;
            cursor_pixels = bgra_of_bitmap(mem_hdc, ii.hbmColor, cw, ch);
          } else {
            cursor_pixels = None;
            cw = 0;
            ch = 0;
          }
        } else if !ii.hbmMask.is_invalid() {
          // 单色光标：mask 高度为 2 倍（AND 区 + XOR 区），转为黑/白像素
          if let Some(bm) = bitmap_dims(ii.hbmMask) {
            cw = bm.0;
            ch = bm.1 / 2;
            cursor_pixels = mono_cursor_pixels(mem_hdc, ii.hbmMask, cw, ch);
          } else {
            cursor_pixels = None;
            cw = 0;
            ch = 0;
          }
        } else {
          cursor_pixels = None;
          cw = 0;
          ch = 0;
        }
        if let Some(px) = cursor_pixels {
          overlay_cursor(
            &mut frame,
            w,
            h,
            ci.ptScreenPos.x - ii.xHotspot as i32,
            ci.ptScreenPos.y - ii.yHotspot as i32,
            &px,
            cw,
            ch,
          );
        }
        if !ii.hbmColor.is_invalid() {
          let _ = unsafe { DeleteObject(HGDIOBJ(ii.hbmColor.0)) };
        }
        if !ii.hbmMask.is_invalid() {
          let _ = unsafe { DeleteObject(HGDIOBJ(ii.hbmMask.0)) };
        }
      }
    }

    unsafe {
      let _ = DeleteObject(HGDIOBJ(hbmp.0));
      let _ = DeleteDC(mem_hdc);
      let _ = ReleaseDC(None, screen_hdc);
    }

    Ok(Frame { w, h, bgra: frame })
  }

  fn bitmap_dims(hbmp: windows::Win32::Graphics::Gdi::HBITMAP) -> Option<(u32, u32)> {
    let mut bm = BITMAP::default();
    let n = unsafe {
      GetObjectW(
        hbmp.into(),
        size_of::<BITMAP>() as i32,
        Some((&mut bm as *mut BITMAP).cast()),
      )
    };
    if n == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
      return None;
    }
    Some((bm.bmWidth as u32, bm.bmHeight as u32))
  }

  /// 单色光标 mask → 黑/白 BGRA 像素（带 alpha）
  fn mono_cursor_pixels(
    hdc: HDC,
    hbmp: windows::Win32::Graphics::Gdi::HBITMAP,
    w: u32,
    h: u32,
  ) -> Option<Vec<u8>> {
    let mut bmi = BITMAPINFO {
      bmiHeader: BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w as i32,
        biHeight: -(h as i32) * 2,
        biPlanes: 1,
        biBitCount: 1,
        biCompression: BI_RGB.0,
        ..Default::default()
      },
      bmiColors: [Default::default()],
    };
    let row = (w as usize).div_ceil(8).div_ceil(4) * 4;
    let mut raw = vec![0u8; row * h as usize * 2];
    let got = unsafe {
      GetDIBits(
        hdc,
        hbmp,
        0,
        h * 2,
        Some(raw.as_mut_ptr() as *mut _),
        &mut bmi,
        DIB_RGB_COLORS,
      )
    };
    if got == 0 {
      return None;
    }
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h as usize {
      for x in 0..w as usize {
        let byte_idx = y * row + x / 8;
        let bit = 7 - (x % 8);
        let and_b = (raw[byte_idx] >> bit) & 1;
        let xor_b = (raw[row * h as usize + byte_idx] >> bit) & 1;
        let di = (y * w as usize + x) * 4;
        if and_b == 0 && xor_b == 0 {
          out[di] = 0;
          out[di + 1] = 0;
          out[di + 2] = 0;
          out[di + 3] = 255;
        } else if and_b == 0 && xor_b == 1 {
          out[di] = 255;
          out[di + 1] = 255;
          out[di + 2] = 255;
          out[di + 3] = 255;
        }
      }
    }
    Some(out)
  }
}

#[cfg(target_os = "macos")]
mod imp {
  use core_graphics::display::CGDisplay;

  /// 一帧 BGRA 像素（top-down 行序）
  pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
  }

  /// 捕获主显示器（CGDisplayCreateImage，像素分辨率）。
  ///
  /// 两点与 Windows 端的差异：
  /// - macOS 没有公开 API 能在截图里合成系统光标，故本帧不含光标；
  /// - 只捕获主屏。CGEvent 的鼠标坐标以主屏左上角为原点、单位为逻辑点，
  ///   主屏截图原点与之天然对齐，多屏时的换算见 input 模块的 display_scale。
  pub fn capture_screen() -> Result<Frame, String> {
    let image = CGDisplay::main()
      .image()
      .ok_or_else(|| "截屏失败（请确认已授予屏幕录制权限）".to_string())?;

    if image.bits_per_pixel() != 32 || image.bits_per_component() != 8 {
      return Err(format!(
        "不支持的屏幕像素格式: {}bpp/{}bpc",
        image.bits_per_pixel(),
        image.bits_per_component()
      ));
    }

    let w = image.width();
    let h = image.height();
    let bpr = image.bytes_per_row();
    let row = w * 4;
    let data = image.data();
    let bytes = data.bytes();
    if w == 0 || h == 0 || bpr < row || bytes.len() < bpr * h {
      return Err("截屏数据尺寸异常".to_string());
    }

    // CGDisplayCreateImage 返回 32 位小端 BGRA（premultiplied first），
    // 与 encode::bgra_to_jpeg 期望的内存序一致；逐行拷贝以去掉行尾填充。
    let mut bgra = vec![0u8; row * h];
    for y in 0..h {
      let start = y * bpr;
      bgra[y * row..(y + 1) * row].copy_from_slice(&bytes[start..start + row]);
    }
    Ok(Frame {
      w: w as u32,
      h: h as u32,
      bgra,
    })
  }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod imp {
  pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
  }

  pub fn capture_screen() -> Result<Frame, String> {
    Err("仅支持 Windows / macOS".to_string())
  }
}

pub use imp::capture_screen;
