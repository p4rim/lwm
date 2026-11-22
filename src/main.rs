use std::process::Command;
use std::error::Error;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux,
    EventMask,

};

use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::protocol::xproto::{
    ConnectionExt, 
    Window, 
    ConfigureWindowAux,
    ModMask,
    GrabMode,
    AtomEnum,
    ClientMessageEvent,
    InputFocus,
    NotifyMode,
    KeyButMask,
};

struct WindowManager {
    windows: Vec<Window>,
    focused: Option<Window>,
    screen_width: u16,
    screen_height: u16,
}

impl WindowManager {
    fn tile(&self,conn: &RustConnection,) -> Result<(), Box<dyn Error>> {
        let count = self.windows.len();

        if count == 0 {
            return Ok(());
        }

        let width = u32::from(self.screen_width);
        let height= u32::from(self.screen_height);

        for (i, &Window) in self.windows.iter().enumerate() {
            let left = width * i as u32 / count as u32;
            let right = width * (i as u32 + 1) / count as u32;

            let border = 2u32;

            let values = ConfigureWindowAux::new()
                .x(left as i32)
                .y(0)
                .width((right - left).saturating_sub(border * 2).max(1))
                .height(height.saturating_sub(border * 2).max(1))
                .border_width(border);

            conn.configure_window(Window, &values)?;
        }

        conn.flush()?;
        Ok(())
    }

    fn focus_window(&mut self, conn: &RustConnection, window: Window) -> Result<(), Box<dyn Error>> {
        if !self.windows.contains(&window) {
            return Ok(());
        }

        if self.focused == Some(window) {
            return Ok(());
        }

        // Remove the highlight from the previously focused window.
        if let Some(old) = self.focused {
            if old != window {
                conn.change_window_attributes(
                    old,
                    &ChangeWindowAttributesAux::new()
                        .border_pixel(0x333333),
                )?;
            }
        }

        // Tell X11 where keyboard input should go.
        conn.set_input_focus(
            InputFocus::POINTER_ROOT,
            window,
            x11rb::CURRENT_TIME,
        )?;

        // Highlight the newly focused window.
        conn.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new()
                .border_pixel(0x80B4FA),
        )?;

        self.focused = Some(window);

        conn.flush()?;
        Ok(())
    }

    fn focus_next(&mut self, conn: &RustConnection) -> Result<(), Box<dyn Error>> {
        if self.windows.is_empty() {
            return Ok(());
        }

        let index = self
            .focused
            .and_then(|focused| {
                self.windows.iter().position(|&w| w == focused)
            })
            .map(|i| (i + 1) % self.windows.len())
            .unwrap_or(0);

        let window = self.windows[index];

        self.focus_window(conn, window)
    }

    fn focus_previous(&mut self, conn: &RustConnection) -> Result<(), Box<dyn Error>> {
        if self.windows.is_empty() {
            return Ok(());
        }

        let len = self.windows.len();

        let index = self
            .focused
            .and_then(|focused| {
                self.windows.iter().position(|&w| w == focused)
            }) 
            .map(|i| (i + len - 1) % len) 
            .unwrap_or(len - 1);

        let window = self.windows[index];

        self.focus_window(conn, window)
    }

    fn close_focused(&self, conn: &RustConnection) -> Result<(), Box<dyn Error>> {
        let Some(Window) = self.focused else {
            return Ok(());
        };

        // Get the X11 atoms used for graceful closing.
        let wm_protocols = conn
            .intern_atom(false, b"WM_PROTOCOLS")?
            .reply()?
            .atom;

        let wm_delete = conn
            .intern_atom(false, b"WM_DELETE_WINDOW")?
            .reply()?
            .atom;

        // Check whether the application supports WM_DELETE_WINDOW.
        let protocols = conn.get_property(
            false,
            Window,
            wm_protocols,
            AtomEnum::ATOM,
            0,
            u32::MAX,
        )?.reply()?;

        let supports_delete = protocols
            .value32()
            .map(|mut atoms| atoms.any(|atom| atom == wm_delete))
            .unwrap_or(false);

        if supports_delete {
            let event = ClientMessageEvent::new(
                32,
                Window,
                wm_protocols,
                [wm_delete, x11rb::CURRENT_TIME, 0, 0, 0],
            );

            conn.send_event(
                false,
                Window,
                EventMask::NO_EVENT,
                event,
            )?;

            conn.flush()?;
        } else {
            eprintln!("Window {Window} does not support WM_DELETE_WINDOW");
        }

        Ok(())
    }
}

fn spawn_terminal() {
    if let Err(error) = Command::new("ghostty").spawn() {
        eprintln!("Failed to launch terminal: {error}");
    }
}

fn spawn_browser() {
    if let Err(error) = Command::new("chromium").spawn() {
        eprintln!("Failed to launch browser: {error}");
    }
}

fn main() -> Result<(), Box<dyn Error>>{
    // Connect to the X server
    let (conn, screen_num) = RustConnection::connect(None)?;

    // Find the window root
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    println!("lwm: connected to X11");
    println!("lwm: root window = {}", root);

    let mut wm = WindowManager {
        windows: Vec::new(),
        focused: None,
        screen_width: screen.width_in_pixels,
        screen_height: screen.height_in_pixels,
    };

    // Ask X11 to send window management events to us
    let values = ChangeWindowAttributesAux::new().event_mask(
        EventMask::SUBSTRUCTURE_REDIRECT
        | EventMask::SUBSTRUCTURE_NOTIFY,
    ); 

    // Only one WM can own SubstructureRedirect.
    // Check the request so we detect another WM.
    conn.change_window_attributes(root, &values)?
        .check()?;

    // Keycodes for shortcuts 
    let h_keycode = 43;
    let j_keycode = 44;
    let k_keycode = 45;
    let l_keycode = 46;
    let q_keycode = 24;
    let enter_keycode = 36;
    let b_keycode = 56;

    let keycodes = [h_keycode, j_keycode, k_keycode, l_keycode, q_keycode, enter_keycode, b_keycode];

    for keycode in keycodes {
        conn.grab_key(
            false,
            root,
            ModMask::M4,
            keycode,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )?
        .check()?;
    }

    conn.flush()?;

    println!("lwm: running!");

    // Main lwm event loop
    loop {
        let event = conn.wait_for_event()?;

        match event {
            Event::MapRequest(event) => {
                println!("New window: {}", event.window);

                if !wm.windows.contains(&event.window) {
                    wm.windows.push(event.window);
                }

                // Listen for the mouse entering this window.
                conn.change_window_attributes(
                    event.window,
                    &ChangeWindowAttributesAux::new()
                        .event_mask(EventMask::ENTER_WINDOW),
                )?;

                conn.map_window(event.window)?;

                wm.tile(&conn)?;
                wm.focus_window(&conn, event.window)?;
            }

            Event::ConfigureRequest(event) => {
                // Managed windows are controlled by our tiling layout.
                // Unmanaged windows can configure themselves.
                if !wm.windows.contains(&event.window) {
                    let values = 
                        ConfigureWindowAux::from_configure_request(&event);


                    conn.configure_window(event.window, &values)?;
                }
            }

            Event::DestroyNotify(event) => {
                let was_focused = wm.focused == Some(event.window);

                wm.windows.retain(|&w| w != event.window);

                if was_focused {
                    wm.focused = None;
                }

                wm.tile(&conn)?;

                if was_focused {
                    if let Some(&next) = wm.windows.last() {
                        wm.focus_window(&conn, next)?;
                    } else {
                        conn.set_input_focus(
                            InputFocus::POINTER_ROOT,
                            root,
                            x11rb::CURRENT_TIME,
                        )?;
                    }
                }
            }

            Event::UnmapNotify(event) => {
                if wm.windows.contains(&event.window) {
                    wm.windows.retain(|&w| w != event.window);
                    wm.tile(&conn)?;
                }
            }

            Event::EnterNotify(event) => {
                if wm.windows.contains(&event.event) {
                    wm.focus_window(&conn, event.event)?;
                } 

                if event.mode == NotifyMode::NORMAL && wm.windows.contains(&event.event) {
                    wm.focus_window(&conn, event.event)?;
                }
            }


            Event::KeyPress(event) => {
                if event.state.contains(KeyButMask::MOD4) {
                    match event.detail {
                        36 => spawn_terminal(),
                        56 => spawn_browser(),
                        46 => wm.focus_next(&conn)?,
                        43 => wm.focus_previous(&conn)?,
                        24 => wm.close_focused(&conn)?,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        conn.flush()?;
    }
}
