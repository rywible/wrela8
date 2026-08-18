//! Product-host isolation and reservation profile selection.

use crate::VmmError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostProfile {
    Diagnostic,
    Product,
}

impl HostProfile {
    pub(crate) fn selected() -> Result<Self, VmmError> {
        match std::env::var("WRELA_HOST_PROFILE").as_deref() {
            Err(std::env::VarError::NotPresent) | Ok("diagnostic") => Ok(Self::Diagnostic),
            Ok("product") => Ok(Self::Product),
            Ok(other) => Err(VmmError::HostProfile(format!(
                "unknown WRELA_HOST_PROFILE `{other}`; expected diagnostic or product"
            ))),
            Err(std::env::VarError::NotUnicode(_)) => Err(VmmError::HostProfile(
                "WRELA_HOST_PROFILE is not valid Unicode".into(),
            )),
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Diagnostic => "diagnostic-nonconforming",
            Self::Product => "product",
        }
    }

    pub(crate) fn validate_process(self) -> Result<(), VmmError> {
        if self == Self::Diagnostic {
            return Ok(());
        }
        validate_product_process()
    }
}

#[cfg(target_os = "linux")]
fn validate_product_process() -> Result<(), VmmError> {
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|error| VmmError::HostProfile(format!("read process status: {error}")))?;
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name).map(str::trim))
            .ok_or_else(|| VmmError::HostProfile(format!("process status lacks `{name}`")))
    };
    if field("Uid:")?.split_whitespace().next() == Some("0") {
        return Err(VmmError::HostProfile(
            "product Linux VMM must not run as root".into(),
        ));
    }
    let uid = field("Uid:")?
        .split_whitespace()
        .next()
        .ok_or_else(|| VmmError::HostProfile("process status has no real uid".into()))?;
    let passwd = std::fs::read_to_string("/etc/passwd")
        .map_err(|error| VmmError::HostProfile(format!("read passwd database: {error}")))?;
    let dedicated_user = passwd.lines().any(|line| {
        let columns = line.split(':').collect::<Vec<_>>();
        columns.len() >= 4 && columns[0] == "wrela" && columns[2] == uid
    });
    if !dedicated_user {
        return Err(VmmError::HostProfile(
            "product Linux VMM must run as the dedicated `wrela` user".into(),
        ));
    }
    if field("CapEff:")? != "0000000000000000" {
        return Err(VmmError::HostProfile(
            "product Linux VMM retains effective capabilities".into(),
        ));
    }
    if field("Seccomp:")? != "2" {
        return Err(VmmError::HostProfile(
            "product Linux VMM is not under a seccomp filter".into(),
        ));
    }
    if field("Seccomp_filters:")?
        .parse::<u32>()
        .ok()
        .filter(|count| *count >= 2)
        .is_none()
    {
        return Err(VmmError::HostProfile(
            "product Linux VMM lacks the composed syscall/address-family filters".into(),
        ));
    }
    if field("NoNewPrivs:")? != "1" {
        return Err(VmmError::HostProfile(
            "product Linux VMM can acquire new privileges".into(),
        ));
    }
    if field("Cpus_allowed_list:")? != "0-3" {
        return Err(VmmError::HostProfile(
            "product Linux VMM CPU affinity differs from the sealed four-core scope".into(),
        ));
    }
    let pid_is_namespace_init = field("Pid:")? == "1";
    let nested_pid_visible = field("NSpid:")?.split_whitespace().count() >= 2;
    let init_status = std::fs::read_to_string("/proc/1/status")
        .map_err(|error| VmmError::HostProfile(format!("read namespace init status: {error}")))?;
    let init_uid = init_status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:")?.split_whitespace().next())
        .ok_or_else(|| VmmError::HostProfile("namespace init status lacks `Uid:`".into()))?;
    let own_uid = field("Uid:")?.split_whitespace().next().unwrap_or_default();
    let namespace_init_owned_by_user = init_uid == own_uid && own_uid != "0";
    if !pid_is_namespace_init && !nested_pid_visible && !namespace_init_owned_by_user {
        return Err(VmmError::HostProfile(
            "product Linux VMM is not in a private PID namespace".into(),
        ));
    }
    let devices = std::fs::read_to_string("/proc/net/dev")
        .map_err(|error| VmmError::HostProfile(format!("read network namespace: {error}")))?;
    if devices.lines().skip(2).any(|line| {
        line.split_once(':')
            .is_some_and(|(name, _)| name.trim() != "lo")
    }) {
        return Err(VmmError::HostProfile(
            "product Linux VMM network namespace exposes a non-loopback interface".into(),
        ));
    }
    let cgroup = std::fs::read_to_string("/proc/self/cgroup")
        .map_err(|error| VmmError::HostProfile(format!("read cgroup: {error}")))?;
    let cgroup_path = cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| VmmError::HostProfile("product Linux VMM lacks a cgroup-v2 scope".into()))?;
    let unit = cgroup_path.rsplit('/').next().unwrap_or_default();
    if !authenticated_linux_unit(unit) {
        return Err(VmmError::HostProfile(format!(
            "product Linux VMM is outside an authenticated Wrela scope: `{unit}`"
        )));
    }
    let cgroup_root = std::path::Path::new("/sys/fs/cgroup").join(
        cgroup_path
            .strip_prefix('/')
            .ok_or_else(|| VmmError::HostProfile("cgroup-v2 path is not absolute".into()))?,
    );
    for (name, expected) in [
        ("memory.max", "805306368"),
        ("memory.swap.max", "0"),
        ("pids.max", "16"),
        ("cpu.max", "400000 100000"),
    ] {
        let path = cgroup_root.join(name);
        let actual = std::fs::read_to_string(&path)
            .map_err(|error| {
                VmmError::HostProfile(format!("read cgroup control {}: {error}", path.display()))
            })?
            .trim()
            .to_string();
        if actual != expected {
            return Err(VmmError::HostProfile(format!(
                "product Linux VMM cgroup `{name}` is `{actual}`, expected `{expected}`"
            )));
        }
    }
    validate_linux_memlock_limit()?;
    validate_linux_denied_operations()?;
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|error| VmmError::HostProfile(format!("read mount namespace: {error}")))?;
    let root_is_read_only = mountinfo.lines().any(|line| {
        let columns = line.split_whitespace().collect::<Vec<_>>();
        columns.get(4) == Some(&"/")
            && columns
                .get(5)
                .is_some_and(|options| options.split(',').any(|option| option == "ro"))
    });
    if !root_is_read_only {
        return Err(VmmError::HostProfile(
            "product Linux VMM does not have a read-only system filesystem".into(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn authenticated_linux_unit(unit: &str) -> bool {
    if unit == "wrela-vmm.service" {
        return true;
    }
    let stem = unit
        .strip_suffix(".service")
        .or_else(|| unit.strip_suffix(".scope"));
    stem.and_then(|stem| stem.strip_prefix("wrela-run-"))
        .is_some_and(|id| {
            id.len() == 16
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

#[cfg(target_os = "linux")]
fn validate_linux_memlock_limit() -> Result<(), VmmError> {
    #[repr(C)]
    struct Rlimit {
        current: u64,
        maximum: u64,
    }
    unsafe extern "C" {
        fn getrlimit(resource: i32, limit: *mut Rlimit) -> i32;
    }
    const RLIMIT_MEMLOCK: i32 = 8;
    const EXPECTED: u64 = 536_870_912;
    let mut limit = Rlimit {
        current: 0,
        maximum: 0,
    };
    if unsafe { getrlimit(RLIMIT_MEMLOCK, &mut limit) } != 0
        || limit.current != EXPECTED
        || limit.maximum != EXPECTED
    {
        return Err(VmmError::HostProfile(format!(
            "product Linux VMM memlock limit is {}/{}, expected {EXPECTED}/{EXPECTED}",
            limit.current, limit.maximum
        )));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn validate_linux_denied_operations() -> Result<(), VmmError> {
    use std::os::fd::FromRawFd;
    unsafe extern "C" {
        fn socket(domain: i32, kind: i32, protocol: i32) -> i32;
    }
    const AF_INET: i32 = 2;
    const SOCK_STREAM: i32 = 1;
    let socket_fd = unsafe { socket(AF_INET, SOCK_STREAM, 0) };
    if socket_fd >= 0 {
        drop(unsafe { std::fs::File::from_raw_fd(socket_fd) });
        return Err(VmmError::HostProfile(
            "product Linux VMM can create an IPv4 network socket".into(),
        ));
    }
    if !matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(1) | Some(97)
    ) {
        return Err(VmmError::HostProfile(
            "product Linux VMM network denial is not kernel-enforced".into(),
        ));
    }
    let forbidden = std::path::Path::new("/dev/vchiq");
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(forbidden)
    {
        Ok(_) => Err(VmmError::HostProfile(
            "product Linux VMM can open forbidden device /dev/vchiq".into(),
        )),
        // ENOENT is an even narrower device surface than an explicit cgroup
        // denial. If the board exposes the legacy VideoCore device, however,
        // the jail must prove that the kernel refuses it.
        Err(error) if matches!(error.raw_os_error(), Some(1) | Some(2) | Some(13)) => Ok(()),
        Err(_) => Err(VmmError::HostProfile(
            "product Linux VMM device denial is not an explicit permission refusal".into(),
        )),
    }
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub(crate) fn pin_host(profile: HostProfile) -> Result<(), VmmError> {
    pin_current_thread(0, "host", profile)
}

#[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
pub(crate) fn pin_host(_profile: HostProfile) -> Result<(), VmmError> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn validate_product_process() -> Result<(), VmmError> {
    // App Sandbox installs this parameter in every sandboxed process. It is
    // checked in addition to code-signing/package verification; the
    // hypervisor entitlement by itself is deliberately insufficient.
    if std::env::var_os("APP_SANDBOX_CONTAINER_ID").is_none() {
        return Err(VmmError::HostProfile(
            "product macOS VMM is not running inside App Sandbox".into(),
        ));
    }
    validate_macos_signature_and_entitlements()?;
    use std::ffi::c_char;
    unsafe extern "C" {
        fn getpid() -> i32;
        fn sandbox_check(pid: i32, operation: *const c_char, filter: i32, ...) -> i32;
    }
    const NETWORK_OUTBOUND: &[u8] = b"network-outbound\0";
    const FILE_WRITE_DATA: &[u8] = b"file-write-data\0";
    const ROOT: &[u8] = b"/\0";
    // `sandbox_check` returns zero when an operation is allowed. Product
    // evidence requires kernel-enforced denials, so an environment variable
    // copied into an unsandboxed child can never satisfy this check.
    let pid = unsafe { getpid() };
    let network_allowed = unsafe { sandbox_check(pid, NETWORK_OUTBOUND.as_ptr().cast(), 0) == 0 };
    let root_write_allowed = unsafe {
        sandbox_check(
            pid,
            FILE_WRITE_DATA.as_ptr().cast(),
            1,
            ROOT.as_ptr().cast::<c_char>(),
        ) == 0
    };
    if network_allowed || root_write_allowed {
        return Err(VmmError::HostProfile(
            "product macOS VMM lacks the required kernel sandbox denials".into(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn validate_macos_signature_and_entitlements() -> Result<(), VmmError> {
    use std::ffi::{c_char, c_void};
    type CfType = *const c_void;
    type CfString = *const c_void;
    type SecCode = *const c_void;
    type SecRequirement = *const c_void;
    type SecTask = *const c_void;
    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecCodeCopySelf(flags: u32, code: *mut SecCode) -> i32;
        fn SecCodeCopyDesignatedRequirement(
            code: SecCode,
            flags: u32,
            requirement: *mut SecRequirement,
        ) -> i32;
        fn SecCodeCheckValidity(code: SecCode, flags: u32, requirement: SecRequirement) -> i32;
        fn SecRequirementCreateWithString(
            text: CfString,
            flags: u32,
            requirement: *mut SecRequirement,
        ) -> i32;
        fn SecTaskCreateFromSelf(allocator: *const c_void) -> SecTask;
        fn SecTaskCopyValueForEntitlement(
            task: SecTask,
            entitlement: CfString,
            error: *mut *const c_void,
        ) -> CfType;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            text: *const c_char,
            encoding: u32,
        ) -> CfString;
        fn CFGetTypeID(value: CfType) -> usize;
        fn CFBooleanGetTypeID() -> usize;
        fn CFBooleanGetValue(value: CfType) -> u8;
        fn CFRelease(value: CfType);
    }
    struct CfOwned(CfType);
    impl Drop for CfOwned {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CFRelease(self.0) };
            }
        }
    }
    fn entitlement(name: &str) -> Result<bool, VmmError> {
        const UTF8: u32 = 0x0800_0100;
        let c_name = std::ffi::CString::new(name).expect("static entitlement name has no NUL");
        let key =
            CfOwned(unsafe { CFStringCreateWithCString(std::ptr::null(), c_name.as_ptr(), UTF8) });
        if key.0.is_null() {
            return Err(VmmError::HostProfile(format!(
                "create entitlement key `{name}`"
            )));
        }
        let task = CfOwned(unsafe { SecTaskCreateFromSelf(std::ptr::null()) }.cast());
        if task.0.is_null() {
            return Err(VmmError::HostProfile(
                "create Security task for product entitlement validation".into(),
            ));
        }
        let value = CfOwned(unsafe {
            SecTaskCopyValueForEntitlement(task.0.cast(), key.0, std::ptr::null_mut())
        });
        if value.0.is_null() {
            return Ok(false);
        }
        if unsafe { CFGetTypeID(value.0) } != unsafe { CFBooleanGetTypeID() } {
            return Err(VmmError::HostProfile(format!(
                "macOS entitlement `{name}` is not Boolean"
            )));
        }
        Ok(unsafe { CFBooleanGetValue(value.0) } != 0)
    }

    let mut code: SecCode = std::ptr::null();
    if unsafe { SecCodeCopySelf(0, &mut code) } != 0 || code.is_null() {
        return Err(VmmError::HostProfile(
            "executing macOS VMM has no valid code object".into(),
        ));
    }
    let code = CfOwned(code);
    let mut requirement: SecRequirement = std::ptr::null();
    if unsafe { SecCodeCopyDesignatedRequirement(code.0, 0, &mut requirement) } != 0
        || requirement.is_null()
    {
        return Err(VmmError::HostProfile(
            "executing macOS VMM has no designated signing requirement".into(),
        ));
    }
    let requirement = CfOwned(requirement);
    if unsafe { SecCodeCheckValidity(code.0, 0, requirement.0) } != 0 {
        return Err(VmmError::HostProfile(
            "executing macOS VMM does not satisfy its designated signing requirement".into(),
        ));
    }
    const UTF8: u32 = 0x0800_0100;
    let expected_text = std::ffi::CString::new("identifier \"org.wrela.vmm\"")
        .expect("static signing requirement has no NUL");
    let expected_text = CfOwned(unsafe {
        CFStringCreateWithCString(std::ptr::null(), expected_text.as_ptr(), UTF8)
    });
    let mut expected_requirement: SecRequirement = std::ptr::null();
    if expected_text.0.is_null()
        || unsafe { SecRequirementCreateWithString(expected_text.0, 0, &mut expected_requirement) }
            != 0
        || expected_requirement.is_null()
    {
        return Err(VmmError::HostProfile(
            "construct the sealed macOS signing identity requirement".into(),
        ));
    }
    let expected_requirement = CfOwned(expected_requirement);
    if unsafe { SecCodeCheckValidity(code.0, 0, expected_requirement.0) } != 0 {
        return Err(VmmError::HostProfile(
            "executing macOS VMM does not have identifier org.wrela.vmm".into(),
        ));
    }
    for required in [
        "com.apple.security.app-sandbox",
        "com.apple.security.hypervisor",
        "com.apple.security.files.user-selected.read-only",
    ] {
        if !entitlement(required)? {
            return Err(VmmError::HostProfile(format!(
                "executing macOS VMM lacks required entitlement `{required}`"
            )));
        }
    }
    for forbidden in [
        "com.apple.security.network.client",
        "com.apple.security.network.server",
        "com.apple.security.files.user-selected.read-write",
        "com.apple.security.cs.disable-library-validation",
        "com.apple.security.cs.allow-jit",
        "com.apple.security.get-task-allow",
    ] {
        if entitlement(forbidden)? {
            return Err(VmmError::HostProfile(format!(
                "executing macOS VMM carries forbidden entitlement `{forbidden}`"
            )));
        }
    }
    validate_exact_embedded_entitlements()?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn validate_exact_embedded_entitlements() -> Result<(), VmmError> {
    let executable = std::env::current_exe()
        .map_err(|error| VmmError::HostProfile(format!("locate executing macOS VMM: {error}")))?;
    let bytes = std::fs::read(&executable).map_err(|error| {
        VmmError::HostProfile(format!(
            "read executing macOS VMM {}: {error}",
            executable.display()
        ))
    })?;
    let xml = embedded_macho_entitlements(&bytes).map_err(VmmError::HostProfile)?;
    let actual = parse_boolean_entitlements(xml).map_err(VmmError::HostProfile)?;
    let expected = std::collections::BTreeMap::from([
        ("com.apple.security.app-sandbox", true),
        ("com.apple.security.files.user-selected.read-only", true),
        ("com.apple.security.hypervisor", true),
        ("com.apple.security.network.client", false),
        ("com.apple.security.network.server", false),
    ]);
    if actual != expected {
        return Err(VmmError::HostProfile(
            "executing macOS VMM entitlement dictionary is not the exact sealed set".into(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn embedded_macho_entitlements(bytes: &[u8]) -> Result<&str, String> {
    fn le32(bytes: &[u8], offset: usize) -> Result<u32, String> {
        Ok(u32::from_le_bytes(
            bytes
                .get(offset..offset + 4)
                .ok_or("truncated Mach-O integer")?
                .try_into()
                .expect("four-byte slice"),
        ))
    }
    fn be32(bytes: &[u8], offset: usize) -> Result<u32, String> {
        Ok(u32::from_be_bytes(
            bytes
                .get(offset..offset + 4)
                .ok_or("truncated code-signing integer")?
                .try_into()
                .expect("four-byte slice"),
        ))
    }
    const MH_MAGIC_64: u32 = 0xfeed_facf;
    const LC_CODE_SIGNATURE: u32 = 0x1d;
    const CSMAGIC_EMBEDDED_SIGNATURE: u32 = 0xfade_0cc0;
    const CSMAGIC_EMBEDDED_ENTITLEMENTS: u32 = 0xfade_7171;
    const CSSLOT_ENTITLEMENTS: u32 = 5;
    if le32(bytes, 0)? != MH_MAGIC_64 {
        return Err("executing file is not a thin little-endian Mach-O 64 image".into());
    }
    let commands = usize::try_from(le32(bytes, 16)?).map_err(|_| "too many load commands")?;
    let mut command = 32_usize;
    let mut signature = None;
    for _ in 0..commands {
        let kind = le32(bytes, command)?;
        let size = usize::try_from(le32(bytes, command + 4)?)
            .map_err(|_| "Mach-O load command is too large")?;
        if size < 8
            || command
                .checked_add(size)
                .is_none_or(|end| end > bytes.len())
        {
            return Err("Mach-O load command range is invalid".into());
        }
        if kind == LC_CODE_SIGNATURE {
            let offset = usize::try_from(le32(bytes, command + 8)?)
                .map_err(|_| "code signature offset is too large")?;
            let length = usize::try_from(le32(bytes, command + 12)?)
                .map_err(|_| "code signature length is too large")?;
            signature = bytes.get(offset..offset + length);
        }
        command += size;
    }
    let signature = signature.ok_or("Mach-O image lacks LC_CODE_SIGNATURE")?;
    if be32(signature, 0)? != CSMAGIC_EMBEDDED_SIGNATURE {
        return Err("Mach-O code signature is not an embedded SuperBlob".into());
    }
    let count = usize::try_from(be32(signature, 8)?).map_err(|_| "too many signature slots")?;
    for index in 0..count {
        let row = 12 + index * 8;
        if be32(signature, row)? != CSSLOT_ENTITLEMENTS {
            continue;
        }
        let offset = usize::try_from(be32(signature, row + 4)?)
            .map_err(|_| "entitlement slot offset is too large")?;
        if be32(signature, offset)? != CSMAGIC_EMBEDDED_ENTITLEMENTS {
            return Err("entitlement slot has the wrong blob magic".into());
        }
        let length = usize::try_from(be32(signature, offset + 4)?)
            .map_err(|_| "entitlement blob length is too large")?;
        let payload = signature
            .get(offset + 8..offset + length)
            .ok_or("entitlement blob range is invalid")?;
        return std::str::from_utf8(payload)
            .map_err(|_| "embedded entitlements are not UTF-8 XML".into());
    }
    Err("Mach-O code signature lacks an XML entitlement slot".into())
}

#[cfg(target_os = "macos")]
fn parse_boolean_entitlements(xml: &str) -> Result<std::collections::BTreeMap<&str, bool>, String> {
    let mut remaining = xml;
    let mut out = std::collections::BTreeMap::new();
    while let Some(start) = remaining.find("<key>") {
        remaining = &remaining[start + 5..];
        let end = remaining
            .find("</key>")
            .ok_or("embedded entitlement key is unterminated")?;
        let key = &remaining[..end];
        remaining = remaining[end + 6..].trim_start();
        let value = if let Some(rest) = remaining.strip_prefix("<true/>") {
            remaining = rest;
            true
        } else if let Some(rest) = remaining.strip_prefix("<false/>") {
            remaining = rest;
            false
        } else {
            return Err(format!("embedded entitlement `{key}` is not Boolean"));
        };
        if out.insert(key, value).is_some() {
            return Err(format!("embedded entitlement `{key}` is repeated"));
        }
    }
    if out.is_empty() {
        return Err("embedded entitlement dictionary is empty".into());
    }
    Ok(out)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn validate_product_process() -> Result<(), VmmError> {
    Err(VmmError::HostProfile(
        "the product host profile is unsupported on this OS".into(),
    ))
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub(crate) fn pin_vcpu(core: usize, profile: HostProfile) -> Result<(), VmmError> {
    if profile != HostProfile::Product {
        return Ok(());
    }
    let physical = core.checked_add(1).filter(|cpu| *cpu < 4).ok_or_else(|| {
        VmmError::HostProfile(format!("no flagship physical CPU for vCPU {core}"))
    })?;
    pin_current_thread(physical, &format!("vCPU {core}"), profile)
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn pin_current_thread(physical: usize, role: &str, profile: HostProfile) -> Result<(), VmmError> {
    if profile != HostProfile::Product {
        return Ok(());
    }
    const CPU_SET_BYTES: usize = 128;
    unsafe extern "C" {
        fn pthread_self() -> usize;
        fn pthread_setaffinity_np(thread: usize, size: usize, set: *const u8) -> i32;
    }
    let mut set = [0_u8; CPU_SET_BYTES];
    set[physical / 8] |= 1 << (physical % 8);
    let result = unsafe { pthread_setaffinity_np(pthread_self(), set.len(), set.as_ptr()) };
    if result != 0 {
        return Err(VmmError::HostProfile(format!(
            "pin {role} to physical core {physical}: errno {result}"
        )));
    }
    Ok(())
}

#[cfg(not(all(target_os = "linux", target_arch = "aarch64")))]
pub(crate) fn pin_vcpu(_core: usize, _profile: HostProfile) -> Result<(), VmmError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names_keep_diagnostics_nonconforming() {
        assert_eq!(HostProfile::Diagnostic.name(), "diagnostic-nonconforming");
        assert_eq!(HostProfile::Product.name(), "product");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn product_unit_authentication_is_exact_for_service_and_scope_launches() {
        for accepted in [
            "wrela-vmm.service",
            "wrela-run-0123456789abcdef.service",
            "wrela-run-0123456789abcdef.scope",
        ] {
            assert!(authenticated_linux_unit(accepted), "{accepted}");
        }
        for refused in [
            "wrela.service",
            "wrela-run-.service",
            "wrela-run-0123456789abcde.service",
            "wrela-run-0123456789abcdef0.service",
            "wrela-run-0123456789abcdeg.service",
            "wrela-run-0123456789abcdef.timer",
        ] {
            assert!(!authenticated_linux_unit(refused), "{refused}");
        }
    }
}
