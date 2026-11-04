//! 真机双屏取证（诊断二进制，不入正式产物）
//!
//! 目的：用【本机真实显示器工作区】+【应用真实几何纯函数】验证 FR-022：
//! 悬浮窗在任意显示器上吸附/钳制/居中，结果都必须落在【该屏工作区内】，
//! 不得跳到其它屏（尤其不得跳回主屏）。
//!
//! 运行：cargo run --release --bin verify_dual_screen
//! 退出码：0 = 全部通过；1 = 存在失败项

use audioshare_lib::float_window::{
    anchor_adjust, clamp_to_work_area, default_center, snap_to_edge, Orientation, Rect, SNAP_THRESHOLD,
};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};

/// 枚举本机全部显示器的（物理屏幕 rcMonitor，工作区 rcWork），均为物理像素
fn list_screens() -> Vec<(Rect, Rect)> {
    thread_local! {
        static SCREENS: std::cell::RefCell<Vec<(Rect, Rect)>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    unsafe extern "system" fn cb(
        hmon: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        _lparam: LPARAM,
    ) -> windows::core::BOOL {
        let mut info = MONITORINFO::default();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut info).as_bool() {
            let monitor = Rect {
                x: info.rcMonitor.left as f64,
                y: info.rcMonitor.top as f64,
                w: (info.rcMonitor.right - info.rcMonitor.left) as f64,
                h: (info.rcMonitor.bottom - info.rcMonitor.top) as f64,
            };
            let work = Rect {
                x: info.rcWork.left as f64,
                y: info.rcWork.top as f64,
                w: (info.rcWork.right - info.rcWork.left) as f64,
                h: (info.rcWork.bottom - info.rcWork.top) as f64,
            };
            SCREENS.with(|w| w.borrow_mut().push((monitor, work)));
        }
        windows::core::BOOL(1)
    }
    unsafe {
        EnumDisplayMonitors(None, None, Some(cb), LPARAM(0));
    }
    SCREENS.with(|w| w.borrow().clone())
}

fn inside(r: Rect, x: f64, y: f64) -> bool {
    x >= r.x - 0.5 && x <= r.x + r.w + 0.5 && y >= r.y - 0.5 && y <= r.y + r.h + 0.5
}

fn main() {
    let screens = list_screens();
    println!("== 本机显示器（rcMonitor / rcWork，物理像素）==");
    for (i, (m, w)) in screens.iter().enumerate() {
        let kind = if m.x == 0.0 && m.y == 0.0 { "主屏" } else { "副屏" };
        println!(
            "  [{i}] {kind} monitor=({},{}) {}x{}  work=({},{}) {}x{}",
            m.x, m.y, m.w, m.h, w.x, w.y, w.w, w.h
        );
    }
    if screens.len() < 2 {
        println!("RESULT=SKIP 本机仅 {} 块显示器，无法验证双屏", screens.len());
        return;
    }

    let icons = 3usize; // 与前端 MAX_VISIBLE_ICONS 一致
    let mut fail = 0usize;
    let mut total = 0usize;

    // ---------- 1) 逐屏逐边吸附：结果必须落在【本屏】内（贴底停在任务栏区域，仍属本屏物理屏） ----------
    println!("\n== 吸附验证（拖到各屏边缘释放）==");
    for (i, (monitor, work)) in screens.iter().enumerate() {
        let cases: Vec<(&str, Rect, Orientation)> = vec![
            (
                "贴左",
                Rect { x: work.x + 8.0, y: work.y + 200.0, w: 299.0, h: 44.0 },
                Orientation::Horizontal,
            ),
            (
                "贴上",
                Rect { x: work.x + 200.0, y: work.y + 5.0, w: 299.0, h: 44.0 },
                Orientation::Horizontal,
            ),
            (
                "贴右",
                Rect { x: work.x + work.w - 299.0 - 8.0, y: work.y + 200.0, w: 299.0, h: 44.0 },
                Orientation::Horizontal,
            ),
            (
                // 贴下：距【屏幕物理底】（rcMonitor）8px 才吸附，目标 = 物理底而非任务栏上沿
                "贴下",
                Rect { x: work.x + 200.0, y: monitor.y + monitor.h - 44.0 - 8.0, w: 299.0, h: 44.0 },
                Orientation::Horizontal,
            ),
            (
                "屏内中部",
                Rect { x: work.x + 50.0, y: work.y + 50.0, w: 299.0, h: 44.0 },
                Orientation::Horizontal,
            ),
        ];
        for (name, rect, cur) in cases {
            total += 1;
            let r = snap_to_edge(rect, *work, *monitor, SNAP_THRESHOLD, cur, icons);
            let r = anchor_adjust(rect, r, cur, *work, icons);
            // 吸附结果必须在本屏【物理屏幕】内，且不落入其它屏的物理屏幕
            let in_self = inside(*monitor, r.x, r.y);
            let in_other = screens
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .any(|(_, (m, _))| inside(*m, r.x, r.y));
            let ok = in_self && !in_other;
            if !ok {
                fail += 1;
            }
            println!(
                "  [{}] 屏{} {}: 拖放前=({},{}) → 结果=({},{}) 方向={:?} 尺寸={}x{}  本屏内={} 落入其它屏={}  {}",
                if ok { "PASS" } else { "FAIL" },
                i,
                name,
                rect.x,
                rect.y,
                r.x,
                r.y,
                r.orientation,
                r.w,
                r.h,
                in_self,
                in_other,
                if ok { "" } else { "<== 越屏！" }
            );
        }
    }

    // ---------- 2) 越界钳制：必须落回本屏工作区 ----------
    println!("\n== 越界钳制验证 ==");
    for (i, (monitor, work)) in screens.iter().enumerate() {
        total += 1;
        let (cx, cy) = clamp_to_work_area(work.x - 5000.0, work.y - 5000.0, 299.0, 44.0, *work, *monitor);
        let ok = inside(*work, cx, cy);
        if !ok {
            fail += 1;
        }
        println!(
            "  [{}] 屏{i}: 钳制结果=({},{}) 期望=({},{})",
            if ok { "PASS" } else { "FAIL" },
            cx,
            cy,
            work.x,
            work.y
        );
    }

    // ---------- 3) 工作区居中：必须落在本屏 ----------
    println!("\n== 默认居中验证 ==");
    for (i, (_, work)) in screens.iter().enumerate() {
        total += 1;
        let (cx, cy) = default_center(*work, 299.0, 44.0);
        let ok = inside(*work, cx, cy);
        if !ok {
            fail += 1;
        }
        println!(
            "  [{}] 屏{i}: 居中=({},{}) 工作区=({},{}) {}x{}",
            if ok { "PASS" } else { "FAIL" },
            cx,
            cy,
            work.x,
            work.y,
            work.w,
            work.h
        );
    }

    // ---------- 4) 跨屏不重算尺寸 ----------
    println!("\n== 尺寸口径（与屏幕无关）==");
    let mut combos: Vec<(u32, u32)> = Vec::new();
    for n in 0..=3usize {
        let (hw, hh) = audioshare_lib::float_window::target_size(Orientation::Horizontal, n);
        let (vw, vh) = audioshare_lib::float_window::target_size(Orientation::Vertical, n);
        combos.push((hw as u32, hh as u32));
        combos.push((vw as u32, vh as u32));
        println!(
            "  应用数={n}: 水平 {hw}x{hh} / 垂直 {vw}x{vh} → ({},{}),({},{})",
            hw as u32, hh as u32, vw as u32, vh as u32
        );
    }
    combos.sort_unstable();
    let mut dupes: Vec<(u32, u32)> = Vec::new();
    for w in combos.windows(2) {
        if w[0] == w[1] && !dupes.contains(&w[0]) {
            dupes.push(w[0]);
        }
    }
    total += 1;
    let size_ok = dupes.is_empty(); // 8 种组合互不相同
    if !size_ok {
        fail += 1;
    }
    println!("  组合 {} 个，重复 {:?}", combos.len(), dupes);
    println!(
        "  [{}] 尺寸档位互不冲突（{} 种独立组合）",
        if size_ok { "PASS" } else { "FAIL" },
        combos.len() - dupes.len()
    );

    println!("\n共 {total} 项，失败 {fail} 项");
    if fail == 0 {
        println!("RESULT=PASS 双屏吸附/钳制/居中 全部落在本屏内（贴底停在物理屏底）");
    } else {
        println!("RESULT=FAIL 有 {fail} 项越屏");
    }
    std::process::exit(if fail == 0 { 0 } else { 1 });
}
