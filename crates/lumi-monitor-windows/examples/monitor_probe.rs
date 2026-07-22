use lumi_monitor_windows::{MonitorBackend, WindowsMonitorBackend};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let monitors = WindowsMonitorBackend.enumerate()?;
    println!("qualified monitors: {}", monitors.len());
    for monitor in monitors {
        println!(
            "{} | {} | {} | raw={:?} | error={:?}",
            monitor.id,
            monitor.display_name,
            monitor.display_path,
            monitor.brightness,
            monitor.qualification_error
        );
    }
    Ok(())
}
