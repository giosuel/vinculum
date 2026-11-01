//! The files Lituus writes, checked by a real Hyprland (`Hyprland --verify-config`): the one on
//! the PATH, or the command in LITUUS_HYPRLAND (`{dir}` in it is the config folder), such as a
//! container with Hyprland. Skipped when there's neither.

use std::process::Command;

use lituus_hypr::{Monitor, MonitorRule, WorkspaceRule, file};

fn hyprland(dir: &std::path::Path) -> Option<Command> {
    if let Ok(template) = std::env::var("LITUUS_HYPRLAND") {
        let words: Vec<String> =
            template.split_whitespace().map(|w| w.replace("{dir}", &dir.display().to_string())).collect();
        let mut c = Command::new(&words[0]);
        c.args(&words[1..]);
        return Some(c);
    }
    let found = std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .any(|p| std::path::Path::new(p).join("Hyprland").exists());
    found.then(|| {
        let mut c = Command::new("Hyprland");
        let runtime = dir.join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        // Hyprland crashes with a runtime folder others can read.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Hyprland also crashes without a home folder (CI's containers have none).
        c.env("XDG_RUNTIME_DIR", runtime)
            .env("HOME", dir)
            .args(["--i-am-really-stupid", "--verify-config", "-c"])
            .arg(dir.join("hyprland.lua"));
        c
    })
}

#[test]
fn hyprland_accepts_the_lua_file() {
    let dir = std::env::temp_dir().join(format!("lituus-verify-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let Some(mut verify) = hyprland(&dir) else {
        eprintln!("no Hyprland to check with: skipped");
        return;
    };
    let all: Vec<Monitor> = serde_json::from_str(include_str!("fixtures/monitors.json")).unwrap();
    let mut rules: Vec<MonitorRule> = all.iter().map(|m| MonitorRule::from_monitor(m, &all)).collect();
    // Every kind of field Lituus writes.
    rules[1].ten_bit = true;
    rules[1].cm = "hdr".into();
    rules[1].sdr_brightness = 1.2;
    rules[1].sdr_saturation = 0.98;
    rules[1].vrr = Some(2);
    rules[2].transform = 1;
    rules[2].reserved = [40, 0, 0, 0];
    rules[3].mirror = Some(rules[1].output.clone());
    rules.push(MonitorRule { output: "DP-9".into(), disabled: true, ..rules[0].clone() });
    let workspaces = vec![
        WorkspaceRule { workspace: "1".into(), monitor: rules[1].output.clone(), default: true, persistent: true },
        WorkspaceRule { workspace: "4".into(), monitor: rules[0].output.clone(), default: false, persistent: false },
    ];
    let mut text = file(true, Some("Desk + TV"), &rules, &workspaces);
    // The live forms too: they're what `hyprctl eval` runs.
    for r in &rules {
        text += &r.lua_live();
        text += "\n";
    }
    std::fs::write(dir.join("lituus.lua"), &text).unwrap();
    std::fs::write(dir.join("hyprland.lua"), "require(\"lituus\")\n").unwrap();
    let out = verify.output().expect("Hyprland --verify-config");
    let report = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    let errors: Vec<&str> = report.lines().filter(|l| l.contains("lituus.lua")).collect();
    assert!(errors.is_empty() && out.status.success(), "Hyprland found problems ({}):\n{report}\n{text}", out.status);
    std::fs::remove_dir_all(&dir).unwrap();
}
