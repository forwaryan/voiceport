//! Independent, immutable per-revision Keychain references. No Codex authentication access.
use core_foundation::{
    base::{CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
    data::CFData,
    dictionary::CFDictionary,
    string::{CFString, CFStringRef},
};
use std::collections::BTreeMap;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecClass: CFStringRef;
    static kSecClassGenericPassword: CFStringRef;
    static kSecAttrService: CFStringRef;
    static kSecAttrAccount: CFStringRef;
    static kSecValueData: CFStringRef;
    static kSecReturnData: CFStringRef;
    static kSecUseAuthenticationUI: CFStringRef;
    static kSecUseAuthenticationUIFail: CFStringRef;
    fn SecItemAdd(attributes: CFTypeRef, result: *mut CFTypeRef) -> i32;
    fn SecItemCopyMatching(query: CFTypeRef, result: *mut CFTypeRef) -> i32;
    fn SecItemDelete(query: CFTypeRef) -> i32;
}
fn constant(p: CFStringRef) -> CFString {
    unsafe { CFString::wrap_under_get_rule(p) }
}
fn query(reference: &str) -> Vec<(CFString, CFType)> {
    unsafe {
        vec![
            (
                constant(kSecClass),
                constant(kSecClassGenericPassword).as_CFType(),
            ),
            (
                constant(kSecAttrService),
                CFString::new("VoicePort.TranscriptionSources.v1").as_CFType(),
            ),
            (
                constant(kSecAttrAccount),
                CFString::new(reference).as_CFType(),
            ),
            (
                constant(kSecUseAuthenticationUI),
                constant(kSecUseAuthenticationUIFail).as_CFType(),
            ),
        ]
    }
}
pub trait Credentials {
    fn get(&self, reference: &str) -> Result<String, String>;
}
/// Validate a new value without touching Keychain or including it in an error.
pub(crate) fn validate_secret(secret: &str) -> Result<(), String> {
    if secret.len() > 4096 || secret.chars().any(char::is_control) {
        return Err("密钥过长或包含控制字符。".into());
    }
    Ok(())
}
pub struct Keychain;
impl Keychain {
    pub fn create(&self, secret: &str) -> Result<String, String> {
        validate_secret(secret)?;
        let reference = format!("key-{}", uuid::Uuid::new_v4());
        let mut pairs = query(&reference);
        pairs.push((
            unsafe { constant(kSecValueData) },
            CFData::from_buffer(secret.as_bytes()).as_CFType(),
        ));
        let dict = CFDictionary::from_CFType_pairs(&pairs);
        if unsafe { SecItemAdd(dict.as_CFTypeRef(), std::ptr::null_mut()) } != 0 {
            return Err("无法写入此来源的 Keychain 密钥。".into());
        }
        Ok(reference)
    }
    pub fn delete(&self, reference: &str) -> Result<(), String> {
        let dict = CFDictionary::from_CFType_pairs(&query(reference));
        match unsafe { SecItemDelete(dict.as_CFTypeRef()) } {
            0 | -25300 => Ok(()),
            _ => Err("无法删除此来源的 Keychain 密钥。".into()),
        }
    }
}
impl Credentials for Keychain {
    fn get(&self, reference: &str) -> Result<String, String> {
        let mut pairs = query(reference);
        pairs.push((
            unsafe { constant(kSecReturnData) },
            CFBoolean::true_value().as_CFType(),
        ));
        let dict = CFDictionary::from_CFType_pairs(&pairs);
        let mut result = std::ptr::null();
        if unsafe { SecItemCopyMatching(dict.as_CFTypeRef(), &mut result) } != 0 || result.is_null()
        {
            return Err("原来源的密钥已删除、锁定或不可读取，请重新确认配置。".into());
        }
        let value = unsafe { CFType::wrap_under_create_rule(result) };
        let data = value
            .downcast::<CFData>()
            .ok_or("Keychain 返回的密钥类型无效。")?;
        let secret =
            String::from_utf8(data.bytes().to_vec()).map_err(|_| "Keychain 密钥编码无效。")?;
        if secret.len() > 4096 || secret.chars().any(char::is_control) {
            return Err("Keychain 密钥格式无效。".into());
        }
        Ok(secret)
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct Secrets(BTreeMap<String, (String, String)>);
impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secrets([redacted])")
    }
}
impl Secrets {
    pub fn pin(
        refs: &BTreeMap<String, String>,
        credentials: &impl Credentials,
    ) -> Result<Self, String> {
        refs.iter()
            .map(|(field, reference)| {
                Ok((
                    field.clone(),
                    (reference.clone(), credentials.get(reference)?),
                ))
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }
    pub fn check(&self, credentials: &impl Credentials) -> Result<(), String> {
        for (reference, pinned) in self.0.values() {
            if credentials.get(reference)? != *pinned {
                return Err("原来源密钥已被替换，请重新确认配置；未发送音频。".into());
            }
        }
        Ok(())
    }
    pub fn values(&self) -> BTreeMap<String, String> {
        self.0
            .iter()
            .map(|(k, (_, v))| (k.clone(), v.clone()))
            .collect()
    }
    pub fn redact(&self, text: &str) -> String {
        let mut output = text.to_owned();
        for (_, secret) in self.0.values() {
            if !secret.is_empty() {
                output = output.replace(secret, "[redacted]");
            }
        }
        output
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Memory(BTreeMap<String, String>);
    impl Credentials for Memory {
        fn get(&self, r: &str) -> Result<String, String> {
            self.0.get(r).cloned().ok_or("missing".into())
        }
    }
    #[test]
    fn credential_replacement_or_removal_fails_without_real_keychain() {
        let refs = BTreeMap::from([("api_key".into(), "synthetic-ref".into())]);
        let mut memory = Memory(BTreeMap::from([(
            "synthetic-ref".into(),
            "synthetic-secret".into(),
        )]));
        let pin = Secrets::pin(&refs, &memory).unwrap();
        assert!(pin.check(&memory).is_ok());
        assert!(!format!("{pin:?}").contains("synthetic-secret"));
        memory.0.insert("synthetic-ref".into(), "replaced".into());
        assert!(pin.check(&memory).is_err());
        memory.0.clear();
        assert!(pin.check(&memory).is_err());
        assert_eq!(pin.redact("oops synthetic-secret"), "oops [redacted]");
    }
}
