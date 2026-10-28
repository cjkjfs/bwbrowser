//! Windows DXGI Desktop Duplication 采集（快路径）。
//!
//! 相比 GDI BitBlt：GPU 直出帧、无需全屏内存 blit，采集耗时显著降低。
//! 光标不随 GPU 帧给出，用 GetCursorInfo + 两遍提取（黑白底）软件合成，
//! 兼容 ARGB 彩色光标与单色 AND/XOR 掩码光标。
//! 初始化失败（远程会话/驱动不支持）返回 None，调用方回退 GDI。

#![cfg(windows)]

use std::mem::size_of;
use std::sync::Mutex;

use windows::Win32::Graphics::Gdi::{
  DeleteObject, GetDC, GetDIBits, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
  HBITMAP,
};
use windows::Win32::UI::WindowsAndMessaging::{
  GetCursorInfo, GetIconInfo, CURSORINFO, CURSORINFO_FLAGS, ICONINFO,
};

use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
  D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
  D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE,
  D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{
  CreateDXGIFactory1, IDXGIFactory1, IDXGIOutputDuplication, IDXGIResource, DXGI_OUTDUPL_FRAME_INFO,
};

/// 一帧 BGRA（top-down）。
pub struct DxgiFrame {
  pub w: u32,
  pub h: u32,
  pub bgra: Vec<u8>,
}

struct Duplication {
  ctx: ID3D11DeviceContext,
  dup: IDXGIOutputDuplication,
  staging: ID3D11Texture2D,
  w: u32,
  h: u32,
  last_bgra: Vec<u8>,
}

static DXGI: Mutex<Option<Duplication>> = Mutex::new(None);

fn create_duplication() -> Result<Duplication, String> {
  unsafe {
    let mut device: Option<ID3D11Device> = None;
    let mut ctx: Option<ID3D11DeviceContext> = None;
    D3D11CreateDevice(
      None,
      D3D_DRIVER_TYPE_HARDWARE,
      HMODULE::default(),
      D3D11_CREATE_DEVICE_BGRA_SUPPORT,
      None,
      D3D11_SDK_VERSION,
      Some(&mut device),
      None,
      Some(&mut ctx),
    )
    .map_err(|e| format!("D3D11CreateDevice: {e}"))?;
    let device = device.ok_or("D3D11CreateDevice 未返回设备")?;
    let ctx = ctx.ok_or("D3D11CreateDevice 未返回 context")?;

    let factory: IDXGIFactory1 =
      CreateDXGIFactory1().map_err(|e| format!("CreateDXGIFactory1: {e}"))?;
    let mut dup: Option<IDXGIOutputDuplication> = None;
    'adapters: for ai in 0..8u32 {
      let Ok(adapter) = factory.EnumAdapters(ai) else {
        break;
      };
      for oi in 0..8u32 {
        let Ok(out) = adapter.EnumOutputs(oi) else {
          break;
        };
        let Ok(dup1) = out.cast::<windows::Win32::Graphics::Dxgi::IDXGIOutput1>() else {
          continue;
        };
        if let Ok(d) = dup1.DuplicateOutput(&device) {
          dup = Some(d);
          break 'adapters;
        }
      }
    }
    let dup = dup.ok_or("没有可用的 DXGI 输出（远程会话或驱动不支持）")?;

    let ddesc = dup.GetDesc();
    let w = ddesc.ModeDesc.Width;
    let h = ddesc.ModeDesc.Height;
    if w == 0 || h == 0 {
      return Err("DXGI 输出尺寸为 0".into());
    }

    let staging_desc = D3D11_TEXTURE2D_DESC {
      Width: w,
      Height: h,
      MipLevels: 1,
      ArraySize: 1,
      Format: DXGI_FORMAT_B8G8R8A8_UNORM,
      SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
        Count: 1,
        Quality: 0,
      },
      Usage: D3D11_USAGE_STAGING,
      BindFlags: 0,
      CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
      MiscFlags: 0,
    };
    let mut staging: Option<ID3D11Texture2D> = None;
    device
      .CreateTexture2D(&staging_desc, None, Some(&mut staging))
      .map_err(|e| format!("CreateTexture2d: {e}"))?;
    let staging = staging.ok_or("CreateTexture2d 未返回纹理")?;

    Ok(Duplication {
      ctx,
      dup,
      staging,
      w,
      h,
      last_bgra: Vec::new(),
    })
  }
}

/// 采集一帧。返回 None 表示 DXGI 不可用（调用方回退 GDI）。
/// 桌面无更新（Acquire 超时）时复用上次像素。
pub fn capture_dxgi() -> Option<DxgiFrame> {
  let mut guard = DXGI.lock().ok()?;
  if guard.is_none() {
    match create_duplication() {
      Ok(d) => *guard = Some(d),
      Err(_) => return None,
    }
  }
  let state = guard.as_mut()?;
  unsafe {
    let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
    let mut resource: Option<IDXGIResource> = None;
    if state
      .dup
      .AcquireNextFrame(0, &mut info, &mut resource)
      .is_err()
    {
      // 桌面无更新（超时）：复用上次像素，由调用方做像素级静止检测
      if state.last_bgra.is_empty() {
        return None;
      }
      return Some(DxgiFrame {
        w: state.w,
        h: state.h,
        bgra: state.last_bgra.clone(),
      });
    }
    let tex: ID3D11Texture2D = match resource
      .as_ref()
      .and_then(|r| r.cast::<ID3D11Texture2D>().ok())
    {
      Some(t) => t,
      None => {
        let _ = state.dup.ReleaseFrame();
        return None;
      }
    };
    let staging_res: ID3D11Resource = state
      .staging
      .cast()
      .map_err(|e| format!("cast: {e}"))
      .ok()?;
    state.ctx.CopyResource(&staging_res, &tex);
    drop(tex);
    let staging_res: ID3D11Resource = state
      .staging
      .cast()
      .map_err(|e| format!("cast: {e}"))
      .ok()?;
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    state
      .ctx
      .Map(&staging_res, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
      .map_err(|e| format!("Map: {e}"))
      .ok()?;
    let w = state.w as usize;
    let h = state.h as usize;
    let row_pitch = mapped.RowPitch as usize;
    let mut bgra = vec![0u8; w * h * 4];
    for row in 0..h {
      let src = mapped.pData.add(row * row_pitch) as *const u8;
      std::ptr::copy_nonoverlapping(src, bgra.as_mut_ptr().add(row * w * 4), w * 4);
    }
    state.ctx.Unmap(&state.staging, 0);
    let _ = state.dup.ReleaseFrame();
    state.last_bgra = bgra.clone();
    Some(DxgiFrame {
      w: state.w,
      h: state.h,
      bgra,
    })
  }
}

/// 重置（显示器配置变化/分辨率切换后由调用方触发，下次采集自动重建）。
pub fn reset() {
  if let Ok(mut guard) = DXGI.lock() {
    *guard = None;
  }
}

/// 把系统光标合成进 BGRA 帧（DXGI 帧不含光标）。
/// 通过黑白两底各画一遍提取 AND/XOR 掩码，兼容 ARGB 彩色与单色光标。
pub fn blend_cursor(bgra: &mut [u8], w: u32, h: u32) {
  unsafe {
    let mut ci = CURSORINFO {
      cbSize: size_of::<CURSORINFO>() as u32,
      ..Default::default()
    };
    if GetCursorInfo(&mut ci).is_err() || ci.flags == CURSORINFO_FLAGS(0) {
      return;
    }
    let mut icon_ii = ICONINFO::default();
    if GetIconInfo(ci.hCursor.into(), &mut icon_ii).is_err() {
      return;
    }
    let icon = icon_ii;

    // 读 hbmMask / hbmColor 位图
    let mask_hbmp = icon.hbmMask;
    let color_hbmp = icon.hbmColor;
    let bmp_of = |hb: HBITMAP| -> Option<(u32, u32, Vec<u8>, u32)> {
      let mut bm = windows::Win32::Graphics::Gdi::BITMAP::default();
      if unsafe {
        windows::Win32::Graphics::Gdi::GetObjectW(
          hb.into(),
          size_of::<windows::Win32::Graphics::Gdi::BITMAP>() as i32,
          Some(&mut bm as *mut _ as *mut _),
        )
      } == 0
      {
        return None;
      }
      let bw = bm.bmWidth as u32;
      let bh = bm.bmHeight as u32;
      if bw == 0 || bh == 0 {
        return None;
      }
      let hdc = GetDC(None);
      let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
          biSize: size_of::<BITMAPINFOHEADER>() as u32,
          biWidth: bw as i32,
          biHeight: -(bh as i32),
          biPlanes: 1,
          biBitCount: 32,
          biCompression: BI_RGB.0,
          ..Default::default()
        },
        bmiColors: [Default::default()],
      };
      let mut buf = vec![0u8; (bw * bh * 4) as usize];
      let got = GetDIBits(
        hdc,
        hb,
        0,
        bh,
        Some(buf.as_mut_ptr() as *mut _),
        &mut bmi,
        DIB_RGB_COLORS,
      );
      ReleaseDC(None, hdc);
      if got == 0 {
        return None;
      }
      Some((bw, bh, buf, 32))
    };

    let mask_info = bmp_of(mask_hbmp);
    let color_info = if color_hbmp.0.is_null() {
      None
    } else {
      bmp_of(color_hbmp)
    };
    let _ = DeleteObject(mask_hbmp.into());
    if !color_hbmp.0.is_null() {
      let _ = DeleteObject(color_hbmp.into());
    }
    let Some((mw, mh, mask_bits, _)) = mask_info else {
      return;
    };

    let hotspot_x = icon.xHotspot as i64;
    let hotspot_y = icon.yHotspot as i64;
    let x0 = ci.ptScreenPos.x as i64 - hotspot_x;
    let y0 = ci.ptScreenPos.y as i64 - hotspot_y;

    // AND 掩码位（1bpp，行按 32 位对齐）；彩色光标时 mask 只有 AND 行
    let and_row_stride = mw.div_ceil(32) as usize * 4;
    let get_mask_bit = |row: usize, col: usize| -> bool {
      let idx = row * and_row_stride + col / 8;
      idx < mask_bits.len() && (mask_bits[idx] >> (7 - (col % 8))) & 1 == 1
    };

    for cy in 0..mh as i64 {
      let fy = y0 + cy;
      if fy < 0 || fy >= h as i64 {
        continue;
      }
      for cx in 0..mw as i64 {
        let fx = x0 + cx;
        if fx < 0 || fx >= w as i64 {
          continue;
        }
        let dst_i = ((fy as u32 * w + fx as u32) * 4) as usize;
        match &color_info {
          Some((cw2, _ch2, color_bits, _)) => {
            // 彩色光标：BGRA，优先用 alpha；alpha 全 0 时用 AND 掩码判透明
            let src_i = ((cy as u32 * *cw2 + cx as u32) * 4) as usize;
            let (b, g, r, a) = (
              color_bits[src_i],
              color_bits[src_i + 1],
              color_bits[src_i + 2],
              color_bits[src_i + 3],
            );
            if a > 0 {
              let af = a as f32 / 255.0;
              bgra[dst_i] = (b as f32 * af + bgra[dst_i] as f32 * (1.0 - af)) as u8;
              bgra[dst_i + 1] = (g as f32 * af + bgra[dst_i + 1] as f32 * (1.0 - af)) as u8;
              bgra[dst_i + 2] = (r as f32 * af + bgra[dst_i + 2] as f32 * (1.0 - af)) as u8;
            } else if get_mask_bit(cy as usize, cx as usize) {
              // AND=1 → 透明
            } else {
              bgra[dst_i] = b;
              bgra[dst_i + 1] = g;
              bgra[dst_i + 2] = r;
            }
          }
          None => {
            // 单色光标：AND 行 + XOR 行（掩码高度是 2 倍光标高）
            let ch2 = mh / 2;
            if cy >= ch2 as i64 {
              continue;
            }
            let and_bit = get_mask_bit(cy as usize, cx as usize);
            let xor_bit = get_mask_bit(cy as usize + ch2 as usize, cx as usize);
            match (and_bit, xor_bit) {
              (false, false) => {
                bgra[dst_i] = 0;
                bgra[dst_i + 1] = 0;
                bgra[dst_i + 2] = 0;
              }
              (false, true) => {
                bgra[dst_i] = 255;
                bgra[dst_i + 1] = 255;
                bgra[dst_i + 2] = 255;
              }
              (true, true) => {
                // 反色
                bgra[dst_i] = !bgra[dst_i];
                bgra[dst_i + 1] = !bgra[dst_i + 1];
                bgra[dst_i + 2] = !bgra[dst_i + 2];
              }
              (true, false) => {} // 透明
            }
          }
        }
      }
    }
  }
}
