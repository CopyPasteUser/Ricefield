//! Small, explicit locale catalog for shared high-impact file dialogs.
//! English remains the source language; unsupported locales fall back to it.

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Language {
    English,
    German,
}

fn language_from_environment(get: impl Fn(&str) -> Option<String>) -> Language {
    for key in ["LC_ALL", "LC_MESSAGES", "LANGUAGE", "LANG"] {
        if let Some(value) = get(key) {
            if let Some(locale) = value.split(':').next() {
                if locale == "C" || locale == "POSIX" {
                    return Language::English;
                }
                if locale.split(['_', '-', '.']).next() == Some("de") {
                    return Language::German;
                }
                if !locale.is_empty() {
                    return Language::English;
                }
            }
        }
    }
    Language::English
}

pub fn tr(source: &'static str) -> &'static str {
    static LANGUAGE: OnceLock<Language> = OnceLock::new();
    translate(
        *LANGUAGE.get_or_init(|| language_from_environment(|key| std::env::var(key).ok())),
        source,
    )
}

fn translate(language: Language, source: &'static str) -> &'static str {
    if language == Language::English {
        return source;
    }
    match source {
        "New folder" => "Neuer Ordner",
        "New empty file" => "Neue leere Datei",
        "Rename item" => "Element umbenennen",
        "Name" => "Name",
        "Enter a name without slashes, or dots alone." => "Namen ohne Schrägstriche eingeben; nur Punkte sind nicht erlaubt.",
        "Cancel" => "Abbrechen",
        "Save" => "Speichern",
        "Move to Trash?" => "In den Papierkorb verschieben?",
        "Move to Trash" => "In den Papierkorb",
        "Move {count} selected item(s) to Trash?" => "{count} ausgewählte Elemente in den Papierkorb verschieben?",
        "The items can usually be restored from your desktop Trash." => "Die Elemente können normalerweise aus dem Papierkorb wiederhergestellt werden.",
        "Permanently delete items?" => "Elemente endgültig löschen?",
        "Delete {count} item(s) permanently?" => "{count} Elemente endgültig löschen?",
        "This bypasses Trash. There is no undo." => "Der Papierkorb wird umgangen. Dies kann nicht rückgängig gemacht werden.",
        "Delete permanently" => "Endgültig löschen",
        "Unlock encrypted removable volume" => "Verschlüsselten Datenträger entsperren",
        "Enter the volume passphrase. UDisks2 and Polkit handle authorization; RiceField never saves this password." => "Passphrase eingeben. UDisks2 und Polkit übernehmen die Autorisierung; RiceField speichert dieses Passwort nicht.",
        "Volume passphrase" => "Datenträger-Passphrase",
        "Unlock volume" => "Datenträger entsperren",
        "Confirm {action}" => "{action} bestätigen",
        "Unmount all decrypted filesystems before locking this volume." => "Alle entschlüsselten Dateisysteme vor dem Sperren aushängen.",
        "UDisks2 will refuse to lock a busy encrypted mapping." => "UDisks2 verweigert das Sperren eines verwendeten verschlüsselten Datenträgers.",
        "Open files or transfers on this volume may fail after disconnecting it." => "Geöffnete Dateien und Übertragungen können nach dem Trennen fehlschlagen.",
        "Make sure writes are complete before continuing." => "Vor dem Fortfahren sicherstellen, dass alle Schreibvorgänge abgeschlossen sind.",
        "Paste with options" => "Einfügen mit Optionen",
        "Copy" => "kopieren",
        "Move" => "verschieben",
        "{action} {count} item(s) into {destination}" => "{count} Elemente nach {destination} {action}",
        "Keep both — rename incoming items when names collide" => "Beide behalten – neue Elemente bei Namenskonflikten umbenennen",
        "Skip existing — leave any existing destination item untouched (no comparison)" => "Vorhandene überspringen – bestehende Ziele nicht verändern (kein Vergleich)",
        "Require no conflicts — cancel the entire batch if a name collides" => "Keine Konflikte zulassen – gesamten Vorgang bei Namenskonflikt abbrechen",
        "Skip only matching contents — reject different or unverifiable files" => "Nur identische Inhalte überspringen – abweichende oder nicht prüfbare Dateien ablehnen",
        "Start transfer" => "Übertragung starten",
        "Properties" => "Eigenschaften",
        "Reading file properties…" => "Dateieigenschaften werden gelesen…",
        "Close" => "Schließen",
        "Open With" => "Öffnen mit",
        "Choose an installed app for {path}" => "Installierte Anwendung für {path} auswählen",
        "Looking up MIME handlers…" => "Verfügbare Anwendungen werden gesucht…",
        "Choose another app with system dialog…" => "Andere Anwendung über Systemdialog auswählen…",
        "Batch rename ({count} items)" => "Stapel-Umbenennung ({count} Elemente)",
        "Preview every new name before applying the changes." => "Alle neuen Namen vor dem Übernehmen prüfen.",
        "Prefix:" => "Präfix:",
        "Suffix:" => "Suffix:",
        "Find:" => "Suchen:",
        "Replace:" => "Ersetzen:",
        "Regular expression (supports $1 capture replacements)" => "Regulärer Ausdruck (unterstützt $1-Ersetzungen)",
        "Use Rust regular expressions. Names that are not valid UTF-8 cannot use search and replace." => "Rust-Regulärausdrücke verwenden. Ungültige UTF-8-Dateinamen unterstützen Suchen und Ersetzen nicht.",
        "Add sequence numbers" => "Laufende Nummern hinzufügen",
        "Start:" => "Beginn:",
        "Digits:" => "Stellen:",
        "Preview (original → new name)" => "Vorschau (alter → neuer Name)",
        "Targets are checked before renaming. Existing names are never overwritten; if a later rename fails, earlier changes remain." => "Ziele werden vor dem Umbenennen geprüft. Bestehende Namen werden nie überschrieben. Scheitert ein späterer Schritt, bleiben vorherige Änderungen bestehen.",
        "Rename selected" => "Auswahl umbenennen",
        "Connect to network location" => "Mit Netzwerkadresse verbinden",
        "SMB, SFTP, WebDAV or MTP address (no passwords in the URL):" => "SMB-, SFTP-, WebDAV- oder MTP-Adresse (kein Passwort in der URL):",
        "Username (leave empty for server default):" => "Benutzername (leer für Servervorgabe):",
        "NAS username" => "NAS-Benutzername",
        "Domain / workgroup (optional):" => "Domäne / Arbeitsgruppe (optional):",
        "Password (leave empty for guest / existing session):" => "Passwort (leer für Gast / bestehende Sitzung):",
        "NAS password" => "NAS-Passwort",
        "Remember login in Linux keyring" => "Zugang im Linux-Schlüsselbund merken",
        "Only after a successful connection. Requires libsecret-tools and an unlocked Secret Service; never stored in RiceField settings." => "Nur nach erfolgreicher Verbindung. Benötigt libsecret-tools und einen entsperrten Schlüsselbund; niemals in RiceField-Einstellungen gespeichert.",
        "Use saved login" => "Gespeicherten Zugang verwenden",
        "Forget saved login" => "Gespeicherten Zugang vergessen",
        "Passwords are never included in the URL. Saving is opt-in and uses your desktop's encrypted Secret Service." => "Passwörter gehören nicht in URLs. Optionales Speichern nutzt den verschlüsselten Schlüsselbund der Desktop-Sitzung.",
        "Connect" => "Verbinden",
        "Mount" => "Einhängen",
        "Unmount" => "Aushängen",
        "Eject" => "Auswerfen",
        "Unlock encrypted drive" => "Verschlüsselten Datenträger entsperren",
        "Lock encrypted drive" => "Verschlüsselten Datenträger sperren",
        "Power off USB drive" => "USB-Datenträger ausschalten",
        _ => source,
    }
}

pub fn template(key: &'static str, values: &[(&str, String)]) -> String {
    let mut message = tr(key).to_owned();
    for (name, value) in values {
        message = message.replace(&format!("{{{name}}}"), value);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_priority_and_language_fallback() {
        assert_eq!(language_from_environment(|key| match key {
            "LC_MESSAGES" => Some("de_DE.UTF-8".into()),
            "LANG" => Some("en_US.UTF-8".into()),
            _ => None,
        }), Language::German);
        assert_eq!(language_from_environment(|key| (key == "LANG").then(|| "fr_FR".into())), Language::English);
        assert_eq!(translate(Language::German, "Cancel"), "Abbrechen");
        assert_eq!(translate(Language::English, "Cancel"), "Cancel");
        assert_eq!(translate(Language::German, "Untranslated"), "Untranslated");
    }

    #[test]
    fn dialog_templates_substitute_values_without_losing_paths() {
        let message = translate(Language::German, "Delete {count} item(s) permanently?");
        assert_eq!(message.replace("{count}", "2"), "2 Elemente endgültig löschen?");
    }
}
