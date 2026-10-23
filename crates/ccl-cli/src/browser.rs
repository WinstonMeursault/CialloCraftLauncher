//! Default-browser integration belongs to the CLI, not the authentication core.
use std::{process::Stdio, time::Duration};

fn command_for(platform: &str, uri: &str) -> Result<tokio::process::Command, &'static str> {
    let url = reqwest::Url::parse(uri).map_err(|_| "Invalid verification URL")?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !matches!(
            url.host_str(),
            Some(
                "microsoft.com"
                    | "www.microsoft.com"
                    | "login.microsoftonline.com"
                    | "login.live.com"
            )
        )
    {
        return Err("Automatic opening requires a trusted Microsoft HTTPS verification URL");
    }
    // Pass the URL as a single argument. Do not interpolate it into a shell.
    let (program, prefix): (&str, &[&str]) = match platform {
        "macos" => ("/usr/bin/open", &[]),
        "windows" => ("rundll32.exe", &["url.dll,FileProtocolHandler"]),
        "linux" => ("xdg-open", &[]),
        _ => return Err("Automatic browser opening is unavailable on this platform"),
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args(prefix)
        .arg(uri)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    Ok(command)
}

pub async fn open_verification_page(uri: &str) -> Result<(), &'static str> {
    let mut command = command_for(std::env::consts::OS, uri)?;
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start the default browser")?;
    match tokio::time::timeout(Duration::from_secs(3), child.wait()).await {
        Ok(Ok(status)) if status.success() => Ok(()),
        _ => {
            // An unavailable/headless browser must not prevent device login.
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
            Err("Could not open the default browser")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_launchers_receive_a_single_url_argument_without_a_shell() {
        let uri = "https://www.microsoft.com/link";
        for (platform, program, arguments) in [
            ("macos", "/usr/bin/open", vec![uri]),
            (
                "windows",
                "rundll32.exe",
                vec!["url.dll,FileProtocolHandler", uri],
            ),
            ("linux", "xdg-open", vec![uri]),
        ] {
            let command = command_for(platform, uri).unwrap();
            assert_eq!(command.as_std().get_program(), program);
            let actual: Vec<_> = command
                .as_std()
                .get_args()
                .map(|s| s.to_str().unwrap())
                .collect();
            assert_eq!(actual, arguments);
        }
    }

    #[test]
    fn untrusted_verification_destinations_are_not_opened() {
        for uri in [
            "file:///tmp/code",
            "http://www.microsoft.com/link",
            "https://microsoft.com.evil.test/link",
            "https://microsoft.com@evil.test/link",
            "https://evil.test@www.microsoft.com/link",
            "https://www.microsoft.com:8443/link",
            "--help",
        ] {
            assert!(command_for("macos", uri).is_err(), "{uri}");
        }
    }
}
