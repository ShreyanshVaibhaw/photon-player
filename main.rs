slint::include_modules!();

use libmpv_sys::{
    mpv_command, mpv_create, mpv_get_property, mpv_handle, mpv_initialize, mpv_set_option_string,
    mpv_set_property,
};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::{CString, c_void};
use slint::{Timer, TimerMode, ComponentHandle, SharedString};
use std::time::Duration;
use std::rc::Rc;
use std::cell::RefCell;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, MoveWindow, WINDOW_EX_STYLE, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_VISIBLE,
};

const MPV_FORMAT_DOUBLE: u32 = 5;
const CONTROL_BAR_HEIGHT: f32 = 80.0;

struct PlayerState {
    mpv: *mut mpv_handle,
    video_hwnd: HWND,
    ignore_seek_event: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let app = AppWindow::new()?;
    let app_weak = app.as_weak();

    let mpv_ptr = unsafe { mpv_create() };
    if mpv_ptr.is_null() { panic!("Failed to create MPV"); }

    let player_state = Rc::new(RefCell::new(PlayerState { 
        mpv: mpv_ptr, 
        video_hwnd: HWND(0),
        ignore_seek_event: false,
    }));

    let init_timer = Timer::default();
    let update_timer = Timer::default(); 

    let state_for_init = player_state.clone();
    let app_weak_init = app_weak.clone();

    // --- STARTUP ---
    init_timer.start(TimerMode::SingleShot, Duration::from_millis(100), move || {
        let app = app_weak_init.unwrap();
        let mut state = state_for_init.borrow_mut();

        if state.video_hwnd.0 == 0 {
            let slint_handle = app.window().window_handle();
            let handle_wrapper = HasWindowHandle::window_handle(&slint_handle).expect("Window handle missing");
            let parent_hwnd = match handle_wrapper.as_raw() {
                RawWindowHandle::Win32(handle) => HWND(handle.hwnd.get()),
                _ => panic!("Photon Player currently supports only Win32 platforms"),
            };
            let child = unsafe { create_video_child(parent_hwnd) };
            resize_video_child(&app, child);
            state.video_hwnd = child;
        }

        unsafe {
            set_opt(state.mpv, "vo", "gpu");
            set_opt(state.mpv, "hwdec", "auto");
            set_opt(state.mpv, "terminal", "yes");
            set_opt(state.mpv, "keep-open", "yes");
            set_opt(state.mpv, "osd-level", "0");
            set_opt(state.mpv, "osd-bar", "no");
            set_opt(state.mpv, "input-default-bindings", "no");
            set_opt(state.mpv, "input-vo-keyboard", "no");
            
            let wid_str = format!("{}", state.video_hwnd.0);
            set_opt(state.mpv, "wid", &wid_str);
            
            mpv_initialize(state.mpv);
            
            // LOAD INITIAL VIDEO
            let cmd = CString::new("loadfile").unwrap();
            let path = CString::new("C:/test.mp4").unwrap(); 
            let args = [cmd.as_ptr(), path.as_ptr(), std::ptr::null()];
            mpv_command(state.mpv, args.as_ptr() as *mut _);
        }
    });

    let state_for_update = player_state.clone();
    let app_weak_update = app_weak.clone();

    // --- UPDATE LOOP ---
    update_timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
        let app = app_weak_update.unwrap();
        let mut state = state_for_update.borrow_mut();
        resize_video_child(&app, state.video_hwnd);
        
        unsafe {
            let mut pos: f64 = 0.0;
            let name_pos = CString::new("time-pos").unwrap();
            mpv_get_property(state.mpv, name_pos.as_ptr(), MPV_FORMAT_DOUBLE, &mut pos as *mut _ as *mut c_void);
            
            let mut dur: f64 = 1.0;
            let name_dur = CString::new("duration").unwrap();
            mpv_get_property(state.mpv, name_dur.as_ptr(), MPV_FORMAT_DOUBLE, &mut dur as *mut _ as *mut c_void);

            let mut paused: f64 = 0.0;
            let name_pause = CString::new("pause").unwrap();
            mpv_get_property(state.mpv, name_pause.as_ptr(), MPV_FORMAT_DOUBLE, &mut paused as *mut _ as *mut c_void);
            app.set_is_paused(paused > 0.5);

            // Always update the slider position from the backend
            println!("[UPDATE] Setting position to {:.2}s", pos);
            state.ignore_seek_event = true;
            app.set_position(pos as f32);
            state.ignore_seek_event = false;

            if dur > 1.0 {
                app.set_duration(dur as f32);
            }
            
            let time_str = format!("{} / {}", format_time(pos), format_time(dur));
            app.set_time_string(SharedString::from(time_str));
        }
    });

    // --- TOGGLE PAUSE BUTTON ---
    let state_for_pause = player_state.clone();
    app.on_toggle_pause(move || {
        let state = state_for_pause.borrow();
        println!("toggle button pressed");
        
        unsafe {
            let cmd = CString::new("cycle").unwrap();
            let arg = CString::new("pause").unwrap();
            let args = [cmd.as_ptr(), arg.as_ptr(), std::ptr::null()];
            mpv_command(state.mpv, args.as_ptr() as *mut _);
        }
    });

    // --- SEEK VIDEO SLIDER ---
    let state_for_seek = player_state.clone();
    app.on_seek_video(move |position| {


        let mpv_ptr = {
            let mut state = state_for_seek.borrow_mut();
            if state.ignore_seek_event {
                println!("[SEEK] Ignored (backend update)");
                return;
            }
            println!("[SEEK] User seek to {:.2}s", position);
            state.mpv
        };

        unsafe {
            let mut pos_value = position as f64;
            let name = CString::new("time-pos").unwrap();
            let result = mpv_set_property(
                mpv_ptr,
                name.as_ptr(),
                MPV_FORMAT_DOUBLE,
                &mut pos_value as *mut _ as *mut c_void,
            );

            if result < 0 {
                eprintln!("mpv_set_property(time-pos) failed with status {}", result);
            }
        }
    });

    // --- 1. KEYBOARD HANDLER (Space, Arrows) ---
    let state_for_keys = player_state.clone();
    app.on_key_pressed(move |key_text| {
        let state = state_for_keys.borrow();
        let key = key_text.as_str();

        unsafe {
            if key == " " { // Spacebar = Toggle Pause
                let cmd = CString::new("cycle").unwrap();
                let arg = CString::new("pause").unwrap();
                let args = [cmd.as_ptr(), arg.as_ptr(), std::ptr::null()];
                mpv_command(state.mpv, args.as_ptr() as *mut _);
            } 
            else if key == "\u{f703}" || key == "ArrowRight" { // Right Arrow = Forward 5s
                let cmd = CString::new("seek").unwrap();
                let arg = CString::new("5").unwrap(); // +5 seconds
                let mode = CString::new("relative").unwrap();
                let args = [cmd.as_ptr(), arg.as_ptr(), mode.as_ptr(), std::ptr::null()];
                mpv_command(state.mpv, args.as_ptr() as *mut _);
            }
            else if key == "\u{f702}" || key == "ArrowLeft" { // Left Arrow = Back 5s
                let cmd = CString::new("seek").unwrap();
                let arg = CString::new("-5").unwrap(); // -5 Seconds
                let mode = CString::new("relative").unwrap();
                let args = [cmd.as_ptr(), arg.as_ptr(), mode.as_ptr(), std::ptr::null()];
                mpv_command(state.mpv, args.as_ptr() as *mut _);
            }
        }
    });

    app.run()?;
    Ok(())
}

fn format_time(seconds: f64) -> String {
    let total_seconds = seconds as u64;
    let minutes = total_seconds / 60;
    let secs = total_seconds % 60;
    format!("{:02}:{:02}", minutes, secs)
}

unsafe fn set_opt(handle: *mut mpv_handle, name: &str, value: &str) {
    let c_name = CString::new(name).unwrap();
    let c_value = CString::new(value).unwrap();
    mpv_set_option_string(handle, c_name.as_ptr(), c_value.as_ptr());
}

unsafe fn create_video_child(parent_hwnd: HWND) -> HWND {
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        w!("STATIC"),
        PCWSTR::null(),
        WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
        0,
        0,
        0,
        0,
        parent_hwnd,
        None,
        None,
        None,
    );

    if hwnd.0 == 0 {
        panic!("Failed to create mpv child window");
    }

    hwnd
}

fn resize_video_child(app: &AppWindow, video_hwnd: HWND) {
    if video_hwnd.0 == 0 {
        return;
    }

    let size = app.window().size();
    let scale = app.window().scale_factor();
    let control_height_px = (CONTROL_BAR_HEIGHT * scale).round() as i32;
    let width = size.width as i32;
    let height = (size.height as i32 - control_height_px).max(0);

    unsafe {
        let _ = MoveWindow(video_hwnd, 0, 0, width.max(0), height, true);
    }
}