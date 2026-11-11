//! A camera's own device over ONVIF: what it is, and restarting it
//! (Settings → Cameras → General → Restart camera). The address, port and
//! login are the ones on the camera's Pan / tilt / zoom tab.

use crate::onvif::{Onvif, Target, tag_text};
use anyhow::Result;
use serde::Serialize;

const DEVICE: &str = "http://www.onvif.org/ver10/device/wsdl";

#[derive(Debug, Clone, Serialize, Default)]
pub struct DeviceInfo {
    pub manufacturer: String,
    pub model: String,
    pub firmware: String,
}

impl Onvif {
    async fn dev(&self, t: &Target, op: &str, inner: &str) -> Result<String> {
        self.call(
            t,
            &t.device_url(),
            &format!("{DEVICE}/{op}"),
            &format!(r#"<{op} xmlns="{DEVICE}">{inner}</{op}>"#),
        )
        .await
    }

    /// Make and model: also tells whether the camera answers over ONVIF.
    pub async fn device_info(&self, t: &Target) -> Result<DeviceInfo> {
        let x = self.dev(t, "GetDeviceInformation", "").await?;
        let text = |n: &str| tag_text(&x, n).unwrap_or_default();
        Ok(DeviceInfo {
            manufacturer: text("Manufacturer"),
            model: text("Model"),
            firmware: text("FirmwareVersion"),
        })
    }

    pub async fn reboot(&self, t: &Target) -> Result<String> {
        let r = self.dev(t, "SystemReboot", "").await?;
        Ok(tag_text(&r, "Message").unwrap_or_default())
    }
}
