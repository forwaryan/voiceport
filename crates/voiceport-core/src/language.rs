//! Stable speech-language choices shared by native controls, CLI and HTTP validation.
//! A service/model can support a subset; this list is not a quality claim.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("", "自动检测"),
    ("zh", "中文"),
    ("en", "English"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("yue", "粤语"),
    ("af", "Afrikaans"),
    ("am", "አማርኛ"),
    ("ar", "العربية"),
    ("as", "অসমীয়া"),
    ("az", "Azərbaycan"),
    ("ba", "Башҡорт"),
    ("be", "Беларуская"),
    ("bg", "Български"),
    ("bn", "বাংলা"),
    ("bo", "བོད་སྐད་"),
    ("br", "Brezhoneg"),
    ("bs", "Bosanski"),
    ("ca", "Català"),
    ("cs", "Čeština"),
    ("cy", "Cymraeg"),
    ("da", "Dansk"),
    ("de", "Deutsch"),
    ("el", "Ελληνικά"),
    ("es", "Español"),
    ("et", "Eesti"),
    ("eu", "Euskara"),
    ("fa", "فارسی"),
    ("fi", "Suomi"),
    ("fo", "Føroyskt"),
    ("fr", "Français"),
    ("gl", "Galego"),
    ("gu", "ગુજરાતી"),
    ("ha", "Hausa"),
    ("haw", "ʻŌlelo Hawaiʻi"),
    ("he", "עברית"),
    ("hi", "हिन्दी"),
    ("hr", "Hrvatski"),
    ("ht", "Kreyòl ayisyen"),
    ("hu", "Magyar"),
    ("hy", "Հայերեն"),
    ("id", "Bahasa Indonesia"),
    ("is", "Íslenska"),
    ("it", "Italiano"),
    ("jw", "Basa Jawa"),
    ("ka", "ქართული"),
    ("kk", "Қазақша"),
    ("km", "ខ្មែរ"),
    ("kn", "ಕನ್ನಡ"),
    ("la", "Latina"),
    ("lb", "Lëtzebuergesch"),
    ("ln", "Lingála"),
    ("lo", "ລາວ"),
    ("lt", "Lietuvių"),
    ("lv", "Latviešu"),
    ("mg", "Malagasy"),
    ("mi", "Māori"),
    ("mk", "Македонски"),
    ("ml", "മലയാളം"),
    ("mn", "Монгол"),
    ("mr", "मराठी"),
    ("ms", "Bahasa Melayu"),
    ("mt", "Malti"),
    ("my", "မြန်မာ"),
    ("ne", "नेपाली"),
    ("nl", "Nederlands"),
    ("nn", "Nynorsk"),
    ("no", "Norsk"),
    ("oc", "Occitan"),
    ("pa", "ਪੰਜਾਬੀ"),
    ("pl", "Polski"),
    ("ps", "پښتو"),
    ("pt", "Português"),
    ("ro", "Română"),
    ("ru", "Русский"),
    ("sa", "संस्कृतम्"),
    ("sd", "سنڌي"),
    ("si", "සිංහල"),
    ("sk", "Slovenčina"),
    ("sl", "Slovenščina"),
    ("sn", "Shona"),
    ("so", "Soomaali"),
    ("sq", "Shqip"),
    ("sr", "Српски"),
    ("su", "Basa Sunda"),
    ("sv", "Svenska"),
    ("sw", "Kiswahili"),
    ("ta", "தமிழ்"),
    ("te", "తెలుగు"),
    ("tg", "Тоҷикӣ"),
    ("th", "ไทย"),
    ("tk", "Türkmen"),
    ("tl", "Tagalog"),
    ("tr", "Türkçe"),
    ("tt", "Татарча"),
    ("uk", "Українська"),
    ("ur", "اردو"),
    ("uz", "Oʻzbekcha"),
    ("vi", "Tiếng Việt"),
    ("yi", "ייִדיש"),
    ("yo", "Yorùbá"),
];
pub fn supported(code: &str) -> bool {
    LANGUAGES.iter().any(|(value, _)| *value == code)
}
pub fn index(code: &str) -> usize {
    LANGUAGES
        .iter()
        .position(|(value, _)| *value == code)
        .unwrap_or(0)
}
pub fn code(index: usize) -> &'static str {
    LANGUAGES.get(index).map_or("", |(code, _)| code)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn choices_are_unique_and_reject_protocol_injection() {
        let mut codes = std::collections::BTreeSet::new();
        for (i, (value, title)) in LANGUAGES.iter().enumerate() {
            assert!(codes.insert(value));
            assert!(!title.is_empty());
            assert_eq!(index(value), i);
            assert_eq!(code(i), *value);
        }
        for value in ["xx", "JA", "en\r\nmodel=other", "en-US", " en "] {
            assert!(!supported(value));
        }
        for value in ["", "zh", "en", "ja", "ko", "yue", "fr", "ar", "hi"] {
            assert!(supported(value));
        }
    }
}
