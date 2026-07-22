#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use lumi_agent::{AgentHandle, AgentOptions, AgentProcess};
use lumi_ipc::{
    default_instance_name, default_pipe_name, AgentCommand, IpcError, NamedPipeClient,
    SingleInstanceGuard,
};
use std::error::Error;
use std::io;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

fn main() {
    if let Err(error) = run() {
        eprintln!("Lumi Agent failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let background = args.iter().any(|arg| arg == "--background");
    let headless = args
        .iter()
        .any(|arg| arg == "--headless" || arg == "--no-tray");
    let instance_name = default_instance_name()?;
    if args.iter().any(|arg| arg == "--shutdown") {
        shutdown_existing_agent(&instance_name)?;
        return Ok(());
    }
    let _instance = match SingleInstanceGuard::acquire(&instance_name) {
        Ok(instance) => instance,
        Err(IpcError::AlreadyRunning(_)) => {
            if !background {
                request_existing_ui();
            }
            return Ok(());
        }
        Err(error) => return Err(Box::new(error)),
    };

    let process = AgentProcess::start(AgentOptions::production()?)?;
    let handle = process.handle();
    if !background {
        let _ = handle.execute(AgentCommand::OpenUi);
    }
    if headless {
        process.wait();
    } else {
        run_tray(handle)?;
    }
    process.shutdown();
    Ok(())
}

fn request_existing_ui() {
    let Ok(pipe_name) = default_pipe_name() else {
        return;
    };
    let Ok(mut client) = NamedPipeClient::connect(&pipe_name, 1_000) else {
        return;
    };
    let _ = client.call(AgentCommand::OpenUi);
}

fn shutdown_existing_agent(instance_name: &str) -> Result<(), Box<dyn Error>> {
    let connect_deadline = Instant::now() + Duration::from_secs(8);
    let pipe_name = default_pipe_name()?;

    loop {
        match SingleInstanceGuard::acquire(instance_name) {
            Ok(instance) => {
                drop(instance);
                return Ok(());
            }
            Err(IpcError::AlreadyRunning(_)) => {}
            Err(error) => return Err(Box::new(error)),
        }
        if let Ok(mut client) = NamedPipeClient::connect(&pipe_name, 250) {
            client.call(AgentCommand::Shutdown)?;
            break;
        }
        if Instant::now() >= connect_deadline {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out connecting to the running Lumi Agent",
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let exit_deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match SingleInstanceGuard::acquire(instance_name) {
            Ok(instance) => {
                drop(instance);
                return Ok(());
            }
            Err(IpcError::AlreadyRunning(_)) if Instant::now() < exit_deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(IpcError::AlreadyRunning(_)) => {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "timed out waiting for Lumi Agent to exit",
                )));
            }
            Err(error) => return Err(Box::new(error)),
        }
    }
}

#[derive(Clone, Debug)]
enum TrayEvent {
    Open,
    TogglePause,
    SyncPause,
    Quit,
    AgentStopped,
}

struct TrayApplication {
    handle: AgentHandle,
    tray: TrayState,
}

impl ApplicationHandler<TrayEvent> for TrayApplication {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: TrayEvent) {
        match event {
            TrayEvent::Open => {
                let _ = self.handle.execute(AgentCommand::OpenUi);
            }
            TrayEvent::TogglePause => {
                let paused = !self.handle.snapshot().paused;
                if self
                    .handle
                    .execute(AgentCommand::SetPaused { paused })
                    .is_ok()
                {
                    self.tray.set_paused(paused);
                }
            }
            TrayEvent::SyncPause => {
                self.tray.set_paused(self.handle.snapshot().paused);
            }
            TrayEvent::Quit => {
                let _ = self.handle.execute(AgentCommand::Shutdown);
                event_loop.exit();
            }
            TrayEvent::AgentStopped => event_loop.exit(),
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
    }
}

fn run_tray(handle: AgentHandle) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::<TrayEvent>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let tray = TrayState::new(handle.snapshot().paused, proxy.clone())?;
    let shutdown_handle = handle.clone();
    std::thread::Builder::new()
        .name("lumi-tray-shutdown".to_string())
        .spawn(move || {
            shutdown_handle.wait_for_shutdown();
            let _ = proxy.send_event(TrayEvent::AgentStopped);
        })?;
    let mut application = TrayApplication { handle, tray };
    event_loop.run_app(&mut application)?;
    Ok(())
}

struct TrayState {
    _icon: TrayIcon,
    pause_item: MenuItem,
}

impl TrayState {
    fn new(paused: bool, proxy: EventLoopProxy<TrayEvent>) -> Result<Self, Box<dyn Error>> {
        let menu = Menu::new();
        let open = MenuItem::new("Open LumiControl", true, None);
        let pause = MenuItem::new(pause_label(paused), true, None);
        let quit = MenuItem::new("Quit", true, None);
        let open_id = open.id().clone();
        let pause_id = pause.id().clone();
        let quit_id = quit.id().clone();
        menu.append(&open)?;
        menu.append(&pause)?;
        menu.append(&PredefinedMenuItem::separator())?;
        menu.append(&quit)?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("LumiControl")
            .with_icon(make_icon()?)
            .build()?;

        let menu_proxy = proxy.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = if event.id == open_id {
                Some(TrayEvent::Open)
            } else if event.id == pause_id {
                Some(TrayEvent::TogglePause)
            } else if event.id == quit_id {
                Some(TrayEvent::Quit)
            } else {
                None
            };
            if let Some(action) = action {
                let _ = menu_proxy.send_event(action);
            }
        }));

        let last_open = Mutex::new(None::<Instant>);
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            let should_open = matches!(
                &event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if should_open {
                let now = Instant::now();
                let mut previous = last_open
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if previous
                    .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(500))
                {
                    *previous = Some(now);
                    let _ = proxy.send_event(TrayEvent::Open);
                }
            } else if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Right,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                let _ = proxy.send_event(TrayEvent::SyncPause);
            }
        }));

        Ok(Self {
            _icon: tray_icon,
            pause_item: pause,
        })
    }

    fn set_paused(&self, paused: bool) {
        self.pause_item.set_text(pause_label(paused));
    }
}

impl Drop for TrayState {
    fn drop(&mut self) {
        MenuEvent::set_event_handler::<fn(MenuEvent)>(None);
        TrayIconEvent::set_event_handler::<fn(TrayIconEvent)>(None);
    }
}

fn pause_label(paused: bool) -> &'static str {
    if paused {
        "Resume automatic control"
    } else {
        "Pause automatic control"
    }
}

fn make_icon() -> Result<Icon, Box<dyn Error>> {
    let size = 32u32;
    let center = (size as f32 - 1.0) / 2.0;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            let pixel = if distance <= 8.0 {
                [255, 197, 66, 255]
            } else if distance <= 11.0 {
                [84, 169, 255, 230]
            } else {
                [0, 0, 0, 0]
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    Ok(Icon::from_rgba(rgba, size, size)?)
}
