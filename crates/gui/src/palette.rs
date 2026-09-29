use eframe::egui::Color32;

pub const DIRECTORY: Color32 = Color32::from_rgb(0x3a, 0x3a, 0x3a);
pub const HIGHLIGHT: Color32 = Color32::WHITE;

pub fn storage_class(class: &str) -> Color32 {
    match class {
        "STANDARD" => Color32::from_rgb(0x4e, 0x79, 0xa7),
        "INTELLIGENT_TIERING" => Color32::from_rgb(0xb0, 0x7a, 0xa1),
        "STANDARD_IA" => Color32::from_rgb(0x59, 0xa1, 0x4f),
        "ONEZONE_IA" => Color32::from_rgb(0x8c, 0xd1, 0x7d),
        "GLACIER_IR" => Color32::from_rgb(0x76, 0xb7, 0xb2),
        "GLACIER" => Color32::from_rgb(0xf2, 0x8e, 0x2b),
        "DEEP_ARCHIVE" => Color32::from_rgb(0xe1, 0x57, 0x59),
        "EXPRESS_ONEZONE" => Color32::from_rgb(0xed, 0xc9, 0x48),
        "REDUCED_REDUNDANCY" => Color32::from_rgb(0xba, 0xb0, 0xac),
        _ => Color32::from_rgb(0x9c, 0x75, 0x5f),
    }
}
