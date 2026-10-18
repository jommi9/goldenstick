//! Helpers shared by the CLI's unit tests.

use boothready_model::{BusType, PhysicalDevice, Volume};
use std::path::PathBuf;

/// A device with one volume mounted at `mount`.
pub fn device(mount: PathBuf, bus: BusType, removable: bool, is_system: bool) -> PhysicalDevice {
    PhysicalDevice {
        id: "synthetic".into(),
        os_path: "synthetic".into(),
        bus,
        removable,
        is_system,
        size_bytes: 1000,
        logical_sector_size: 512,
        storage_vendor: None,
        storage_model: Some("Stick".into()),
        storage_revision: None,
        usb: None,
        partition_scheme: None,
        volumes: vec![Volume {
            os_path: "synthetic".into(),
            mount_point: Some(mount),
            label: None,
            filesystem: None,
            size_bytes: 1000,
            offset_bytes: None,
            uuid: None,
            efi_system: false,
        }],
    }
}
