//! API keys in Windows Credential Manager (generic credentials, current user only).
//! Never in config.toml, logs or git.

use windows::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
use windows::core::{HSTRING, PWSTR};

fn target(name: &str) -> HSTRING {
    HSTRING::from(format!("Snapling/{name}"))
}

pub fn get(name: &str) -> Option<String> {
    let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        CredReadW(&target(name), CRED_TYPE_GENERIC, None, &mut cred).ok()?;
        let c = &*cred;
        let blob = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        let value = String::from_utf8(blob.to_vec()).ok();
        CredFree(cred.cast());
        value
    }
}

pub fn set(name: &str, value: &str) -> Result<(), String> {
    let mut target_w: Vec<u16> = format!("Snapling/{name}")
        .encode_utf16()
        .chain([0])
        .collect();
    let mut blob = value.as_bytes().to_vec();
    let cred = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(target_w.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    unsafe { CredWriteW(&cred, 0) }.map_err(|e| format!("Credential Manager: {e}"))
}

pub fn delete(name: &str) {
    let _ = unsafe { CredDeleteW(&target(name), CRED_TYPE_GENERIC, None) };
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip() {
        let name = format!("test-{}", std::process::id());
        assert_eq!(super::get(&name), None);
        super::set(&name, "k3y:fx").unwrap();
        assert_eq!(super::get(&name).as_deref(), Some("k3y:fx"));
        super::delete(&name);
        assert_eq!(super::get(&name), None);
    }
}
