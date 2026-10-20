use clap::Parser;

slint::include_modules!();

/// The app ID, also the Wayland app_id that Hyprland window rules match on.
const APP_ID: &str = "ch.giosuel.Vinculum";

#[derive(Parser)]
#[command(name = "vinculum", version = env!("APP_VERSION"), about = "Manage Wi-Fi, wired and VPN connections")]
struct Cli {}

fn main() -> Result<(), slint::PlatformError> {
    Cli::parse();
    let window = MainWindow::new()?;
    // Needs the platform, which creating the first window sets up; takes effect when it's shown.
    slint::set_xdg_app_id(APP_ID)?;
    window.set_version(env!("APP_VERSION").into());
    window.run()
}
