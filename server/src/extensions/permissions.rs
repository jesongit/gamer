//! Extension permission allowlist.
//!
//! This is intentionally a closed enum. Adding a host operation requires a
//! source change and a review instead of silently making a new string usable.

use std::collections::BTreeSet;

use super::error::PermissionError;
use super::host_api::HostApiDomain;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum Permission {
    DeviceRead,
    DeviceApp,
    VisionMatch,
    VisionColor,
    InputTap,
    InputSwipe,
    InputKey,
    InputText,
    Touch,
    ResourceRead,
    RunSubmit,
    RunControl,
    RuntimeSleep,
    LogWrite,
}

impl Permission {
    pub(crate) fn parse(value: &str) -> Result<Self, PermissionError> {
        let value = value.trim();
        match value {
            "device.read" => Ok(Self::DeviceRead),
            "device.app" => Ok(Self::DeviceApp),
            "vision.match" => Ok(Self::VisionMatch),
            "vision.color" => Ok(Self::VisionColor),
            "input.tap" => Ok(Self::InputTap),
            "input.swipe" => Ok(Self::InputSwipe),
            "input.key" => Ok(Self::InputKey),
            "input.text" => Ok(Self::InputText),
            "touch" => Ok(Self::Touch),
            "resource.read" => Ok(Self::ResourceRead),
            "run.submit" => Ok(Self::RunSubmit),
            "run.control" => Ok(Self::RunControl),
            "runtime.sleep" => Ok(Self::RuntimeSleep),
            "log.write" => Ok(Self::LogWrite),
            forbidden
                if forbidden == "filesystem"
                    || forbidden.starts_with("filesystem.")
                    || forbidden == "network"
                    || forbidden.starts_with("network.")
                    || forbidden == "shell"
                    || forbidden.starts_with("shell.")
                    || forbidden == "device.shell"
                    || forbidden.starts_with("device.shell.")
                    || forbidden == "process"
                    || forbidden.starts_with("process.") =>
            {
                Err(PermissionError::Forbidden(forbidden.to_string()))
            }
            other => Err(PermissionError::Unknown(other.to_string())),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::DeviceRead => "device.read",
            Self::DeviceApp => "device.app",
            Self::VisionMatch => "vision.match",
            Self::VisionColor => "vision.color",
            Self::InputTap => "input.tap",
            Self::InputSwipe => "input.swipe",
            Self::InputKey => "input.key",
            Self::InputText => "input.text",
            Self::Touch => "touch",
            Self::ResourceRead => "resource.read",
            Self::RunSubmit => "run.submit",
            Self::RunControl => "run.control",
            Self::RuntimeSleep => "runtime.sleep",
            Self::LogWrite => "log.write",
        }
    }

    pub(crate) fn domain(self) -> HostApiDomain {
        match self {
            Self::DeviceRead | Self::DeviceApp => HostApiDomain::Device,
            Self::VisionMatch | Self::VisionColor => HostApiDomain::Vision,
            Self::InputTap | Self::InputSwipe | Self::InputKey | Self::InputText => {
                HostApiDomain::Input
            }
            Self::Touch => HostApiDomain::Touch,
            Self::ResourceRead => HostApiDomain::Resource,
            Self::RunSubmit | Self::RunControl => HostApiDomain::Run,
            Self::RuntimeSleep => HostApiDomain::Runtime,
            Self::LogWrite => HostApiDomain::Log,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PermissionSet(BTreeSet<Permission>);

impl PermissionSet {
    pub(crate) fn parse<I, S>(values: I) -> Result<Self, PermissionError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut permissions = BTreeSet::new();
        for value in values {
            permissions.insert(Permission::parse(value.as_ref())?);
        }
        Ok(Self(permissions))
    }

    pub(crate) fn allows(&self, permission: Permission) -> bool {
        self.0.contains(&permission)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = Permission> + '_ {
        self.0.iter().copied()
    }

    pub(crate) fn names(&self) -> Vec<&'static str> {
        self.iter().map(Permission::as_str).collect()
    }
}
