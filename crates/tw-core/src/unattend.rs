//! The Windows Setup answer file (`autounattend.xml`) used for the automatic installation.
//!
//! terminal-mac installs macOS through Apple's API and leaves account setup to the user. The
//! Windows equivalent: this answer file automates the installation itself (language, wiping the
//! VM's own empty disk, partitions, edition) and leaves out everything that is the user's to
//! decide: **the license agreement page and the account setup (OOBE) stay interactive.** No
//! product key, no requirement bypass: Windows installs unactivated, exactly like choosing
//! "I don't have a product key".

use crate::host::Arch;

/// A Windows UI language with the matching keyboard (`InputLocale`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Culture {
    pub language: &'static str,
    pub name: &'static str,
    pub input_locale: &'static str,
}

/// Languages whose answer-file values are known. Others fall back to the interactive language page.
pub const CULTURES: &[Culture] = &[
    Culture { language: "English", name: "en-US", input_locale: "0409:00000409" },
    Culture { language: "Korean", name: "ko-KR", input_locale: "0412:00000412" },
    Culture { language: "Japanese", name: "ja-JP", input_locale: "0411:00000411" },
    Culture { language: "German", name: "de-DE", input_locale: "0407:00000407" },
    Culture { language: "French", name: "fr-FR", input_locale: "040c:0000040c" },
    Culture { language: "Chinese Simplified", name: "zh-CN", input_locale: "0804:00000804" },
    Culture { language: "Chinese Traditional", name: "zh-TW", input_locale: "0404:00000404" },
];

pub fn culture_for_language(language: &str) -> Option<&'static Culture> {
    let key: String = language.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
    CULTURES.iter().find(|c| c.language.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>() == key)
}

/// The language of an ISO from its file name. Microsoft's names contain the culture
/// (`Windows11_Client_arm64_ko-kr_26300_9457.iso`); terminal-windows' cache names contain the
/// language (`Win11_26H2_Korean_Arm64.iso`, `..._Chinese_Simplified_...`).
pub fn culture_from_file_name(file_name: &str) -> Option<&'static Culture> {
    let lower = file_name.to_ascii_lowercase();
    CULTURES.iter().find(|c| {
        lower.contains(&format!("_{}_", c.name.to_ascii_lowercase()))
            || lower.contains(&format!("_{}_", c.language.to_ascii_lowercase().replace(' ', "_")))
    })
}

#[derive(Debug, Clone)]
pub struct AnswerFile<'a> {
    pub arch: Arch,
    pub culture: Option<&'a Culture>,
    /// Image name inside the ISO's install.wim; these are not localized.
    pub edition: &'a str,
}

fn arch_name(arch: Arch) -> &'static str {
    match arch {
        Arch::X86_64 => "amd64",
        Arch::Aarch64 => "arm64",
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

impl AnswerFile<'_> {
    pub fn to_xml(&self) -> String {
        let arch = arch_name(self.arch);
        let component = |name: &str| {
            format!(
                "<component name=\"{name}\" processorArchitecture=\"{arch}\" publicKeyToken=\"31bf3856ad364e35\" language=\"neutral\" versionScope=\"nonSxS\" xmlns:wcm=\"http://schemas.microsoft.com/WMIConfig/2002/State\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">"
            )
        };
        let mut x = String::new();
        x.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<unattend xmlns=\"urn:schemas-microsoft-com:unattend\">\n  <settings pass=\"windowsPE\">\n");
        if let Some(c) = self.culture {
            x.push_str(&format!(
                "    {}\n      <SetupUILanguage><UILanguage>{n}</UILanguage></SetupUILanguage>\n      <InputLocale>{i}</InputLocale>\n      <SystemLocale>{n}</SystemLocale>\n      <UILanguage>{n}</UILanguage>\n      <UserLocale>{n}</UserLocale>\n    </component>\n",
                component("Microsoft-Windows-International-Core-WinPE"),
                n = escape(c.name),
                i = escape(c.input_locale)
            ));
        }
        x.push_str(&format!("    {}\n", component("Microsoft-Windows-Setup")));
        x.push_str(
            "      <DiskConfiguration>
        <Disk wcm:action=\"add\">
          <DiskID>0</DiskID>
          <WillWipeDisk>true</WillWipeDisk>
          <CreatePartitions>
            <CreatePartition wcm:action=\"add\"><Order>1</Order><Type>EFI</Type><Size>260</Size></CreatePartition>
            <CreatePartition wcm:action=\"add\"><Order>2</Order><Type>MSR</Type><Size>16</Size></CreatePartition>
            <CreatePartition wcm:action=\"add\"><Order>3</Order><Type>Primary</Type><Extend>true</Extend></CreatePartition>
          </CreatePartitions>
          <ModifyPartitions>
            <ModifyPartition wcm:action=\"add\"><Order>1</Order><PartitionID>1</PartitionID><Format>FAT32</Format><Label>System</Label></ModifyPartition>
            <ModifyPartition wcm:action=\"add\"><Order>2</Order><PartitionID>2</PartitionID></ModifyPartition>
            <ModifyPartition wcm:action=\"add\"><Order>3</Order><PartitionID>3</PartitionID><Format>NTFS</Format><Label>Windows</Label><Letter>C</Letter></ModifyPartition>
          </ModifyPartitions>
        </Disk>
      </DiskConfiguration>
",
        );
        x.push_str(&format!(
            "      <ImageInstall>
        <OSImage>
          <InstallFrom><MetaData wcm:action=\"add\"><Key>/IMAGE/NAME</Key><Value>{}</Value></MetaData></InstallFrom>
          <InstallTo><DiskID>0</DiskID><PartitionID>3</PartitionID></InstallTo>
        </OSImage>
      </ImageInstall>
",
            escape(self.edition)
        ));
        // "I don't have a product key": an empty key and no prompt. Never a real or generic key.
        x.push_str("      <UserData>\n        <ProductKey><Key></Key><WillShowUI>Never</WillShowUI></ProductKey>\n      </UserData>\n");
        x.push_str("    </component>\n  </settings>\n</unattend>\n");
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small well-formedness check: tags balance and attributes are quoted.
    fn assert_well_formed(xml: &str) {
        let mut stack: Vec<String> = Vec::new();
        let mut rest = xml;
        while let Some(start) = rest.find('<') {
            let end = rest[start..].find('>').expect("unterminated tag") + start;
            let tag = &rest[start + 1..end];
            rest = &rest[end + 1..];
            if tag.starts_with('?') {
                continue;
            }
            if let Some(name) = tag.strip_prefix('/') {
                assert_eq!(stack.pop().as_deref(), Some(name), "mismatched closing tag </{name}>");
            } else if !tag.ends_with('/') {
                stack.push(tag.split_whitespace().next().unwrap().to_string());
            }
            assert_eq!(tag.matches('"').count() % 2, 0, "unbalanced quotes in <{tag}>");
        }
        assert!(stack.is_empty(), "unclosed tags: {stack:?}");
    }

    #[test]
    fn answer_file_for_korean_arm64() {
        let xml = AnswerFile { arch: Arch::Aarch64, culture: culture_for_language("Korean"), edition: "Windows 11 Pro" }.to_xml();
        assert_well_formed(&xml);
        assert!(xml.contains("processorArchitecture=\"arm64\""));
        assert!(xml.contains("<UILanguage>ko-KR</UILanguage>") && xml.contains("<InputLocale>0412:00000412</InputLocale>"));
        assert!(xml.contains("<Value>Windows 11 Pro</Value>"));
        assert!(xml.contains("<WillWipeDisk>true</WillWipeDisk>") && xml.contains("<DiskID>0</DiskID>"));
    }

    #[test]
    fn what_stays_with_the_user_is_not_in_the_file() {
        let xml = AnswerFile { arch: Arch::X86_64, culture: None, edition: "Windows 11 Pro" }.to_xml();
        assert_well_formed(&xml);
        assert!(xml.contains("processorArchitecture=\"amd64\""));
        for forbidden in
            ["AcceptEula", "BypassNRO", "BypassTPMCheck", "BypassSecureBootCheck", "LabConfig", "oobeSystem", "LocalAccount", "Password"]
        {
            assert!(!xml.contains(forbidden), "{forbidden} must not be in the answer file");
        }
        // Two <Key> elements only: the image selection and the *empty* product key ("I don't have one").
        assert_eq!(xml.matches("<Key>").count(), 2, "no other key of any kind");
        assert_eq!(xml.matches("<Key>/IMAGE/NAME</Key>").count(), 1);
        assert!(
            xml.contains("<ProductKey><Key></Key><WillShowUI>Never</WillShowUI></ProductKey>"),
            "an empty product key, never a real or generic one"
        );
        assert!(!xml.contains("International-Core-WinPE"), "unknown language leaves the language page interactive");
    }

    #[test]
    fn values_are_escaped() {
        let xml = AnswerFile { arch: Arch::X86_64, culture: None, edition: "A & <B>" }.to_xml();
        assert!(xml.contains("A &amp; &lt;B&gt;"));
        assert_well_formed(&xml);
    }

    #[test]
    fn language_and_file_name_lookup() {
        assert_eq!(culture_for_language("korean").unwrap().name, "ko-KR");
        assert_eq!(culture_for_language("Chinese (Simplified)").unwrap().name, "zh-CN");
        assert!(culture_for_language("Klingon").is_none());
        assert_eq!(culture_from_file_name("Windows11_Client_arm64_ko-kr_26300_9457.iso").unwrap().name, "ko-KR");
        assert_eq!(culture_from_file_name("Win11_26H2_English_Arm64.iso").unwrap().name, "en-US", "cache names use the language");
        assert_eq!(culture_from_file_name("Win11_26H2_Chinese_Simplified_x64.iso").unwrap().name, "zh-CN");
        assert_eq!(culture_from_file_name("Win11_26H2_Chinese_Traditional_x64.iso").unwrap().name, "zh-TW");
        assert!(culture_from_file_name("Win11_26H2_Swedish_Arm64.iso").is_none(), "unknown languages stay interactive");
        assert!(culture_from_file_name("Win11.iso").is_none());
    }
}
