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
            let locale = value.split(':').next().unwrap_or("");
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
        "Enter a name without slashes, or dots alone." => {
            "Namen ohne Schrägstriche eingeben; nur Punkte sind nicht erlaubt."
        }
        "Cancel" => "Abbrechen",
        "Save" => "Speichern",
        "Move to Trash?" => "In den Papierkorb verschieben?",
        "Move to Trash" => "In den Papierkorb",
        "Move {count} selected item(s) to Trash?" => {
            "{count} ausgewählte Elemente in den Papierkorb verschieben?"
        }
        "The items can usually be restored from your desktop Trash." => {
            "Die Elemente können normalerweise aus dem Papierkorb wiederhergestellt werden."
        }
        "Permanently delete items?" => "Elemente endgültig löschen?",
        "Delete {count} item(s) permanently?" => "{count} Elemente endgültig löschen?",
        "This bypasses Trash. There is no undo." => {
            "Der Papierkorb wird umgangen. Dies kann nicht rückgängig gemacht werden."
        }
        "Delete permanently" => "Endgültig löschen",
        "Unlock encrypted removable volume" => "Verschlüsselten Datenträger entsperren",
        "Enter the volume passphrase. UDisks2 and Polkit handle authorization; RiceField never saves this password." => {
            "Passphrase eingeben. UDisks2 und Polkit übernehmen die Autorisierung; RiceField speichert dieses Passwort nicht."
        }
        "Volume passphrase" => "Datenträger-Passphrase",
        "Unlock volume" => "Datenträger entsperren",
        "Confirm {action}" => "{action} bestätigen",
        "Unmount all decrypted filesystems before locking this volume." => {
            "Alle entschlüsselten Dateisysteme vor dem Sperren aushängen."
        }
        "UDisks2 will refuse to lock a busy encrypted mapping." => {
            "UDisks2 verweigert das Sperren eines verwendeten verschlüsselten Datenträgers."
        }
        "Open files or transfers on this volume may fail after disconnecting it." => {
            "Geöffnete Dateien und Übertragungen können nach dem Trennen fehlschlagen."
        }
        "Make sure writes are complete before continuing." => {
            "Vor dem Fortfahren sicherstellen, dass alle Schreibvorgänge abgeschlossen sind."
        }
        "Paste with options" => "Einfügen mit Optionen",
        "Copy" => "kopieren",
        "Move" => "verschieben",
        "{action} {count} item(s) into {destination}" => {
            "{count} Elemente nach {destination} {action}"
        }
        "Keep both — rename incoming items when names collide" => {
            "Beide behalten – neue Elemente bei Namenskonflikten umbenennen"
        }
        "Skip existing — leave any existing destination item untouched (no comparison)" => {
            "Vorhandene überspringen – bestehende Ziele nicht verändern (kein Vergleich)"
        }
        "Require no conflicts — cancel the entire batch if a name collides" => {
            "Keine Konflikte zulassen – gesamten Vorgang bei Namenskonflikt abbrechen"
        }
        "Skip only matching contents — reject different or unverifiable files" => {
            "Nur identische Inhalte überspringen – abweichende oder nicht prüfbare Dateien ablehnen"
        }
        "Start transfer" => "Übertragung starten",
        "Properties" => "Eigenschaften",
        "Reading file properties…" => "Dateieigenschaften werden gelesen…",
        "Close" => "Schließen",
        "Open With" => "Öffnen mit",
        "Choose an installed app for {path}" => "Installierte Anwendung für {path} auswählen",
        "Looking up MIME handlers…" => "Verfügbare Anwendungen werden gesucht…",
        "Choose another app with system dialog…" => {
            "Andere Anwendung über Systemdialog auswählen…"
        }
        "Batch rename ({count} items)" => "Stapel-Umbenennung ({count} Elemente)",
        "Preview every new name before applying the changes." => {
            "Alle neuen Namen vor dem Übernehmen prüfen."
        }
        "Prefix:" => "Präfix:",
        "Suffix:" => "Suffix:",
        "Find:" => "Suchen:",
        "Replace:" => "Ersetzen:",
        "Regular expression (supports $1 capture replacements)" => {
            "Regulärer Ausdruck (unterstützt $1-Ersetzungen)"
        }
        "Use Rust regular expressions. Names that are not valid UTF-8 cannot use search and replace." => {
            "Rust-Regulärausdrücke verwenden. Ungültige UTF-8-Dateinamen unterstützen Suchen und Ersetzen nicht."
        }
        "Add sequence numbers" => "Laufende Nummern hinzufügen",
        "Start:" => "Beginn:",
        "Digits:" => "Stellen:",
        "Preview (original → new name)" => "Vorschau (alter → neuer Name)",
        "Targets are checked before renaming. Existing names are never overwritten; if a later rename fails, earlier changes remain." => {
            "Ziele werden vor dem Umbenennen geprüft. Bestehende Namen werden nie überschrieben. Scheitert ein späterer Schritt, bleiben vorherige Änderungen bestehen."
        }
        "Rename selected" => "Auswahl umbenennen",
        "Connect to network location" => "Mit Netzwerkadresse verbinden",
        "Connect to NAS / network" => "NAS / Netzwerk verbinden",
        "Connect to your NAS" => "NAS verbinden",
        "Enter its IP/hostname, or find a NAS on your local network." => {
            "IP-Adresse oder Hostnamen eingeben oder ein NAS im lokalen Netzwerk finden."
        }
        "SMB / NAS" => "SMB / NAS",
        "Other protocols" => "Weitere Protokolle",
        "Server or IP address" => "Server oder IP-Adresse",
        "Share name (optional)" => "Freigabename (optional)",
        "Leave the share empty to browse available folders first." => {
            "Freigabe leer lassen, um zuerst verfügbare Ordner anzuzeigen."
        }
        "Find NAS on network" => "NAS im Netzwerk suchen",
        "Searching for advertised NAS servers…" => "Suche nach NAS-Geräten im Netzwerk…",
        "No NAS found. Enter its IP address manually." => {
            "Kein NAS gefunden. IP-Adresse manuell eingeben."
        }
        "Network address" => "Netzwerkadresse",
        "SFTP, WebDAV and MTP use a full network URI." => {
            "Für SFTP, WebDAV und MTP eine vollständige Netzwerkadresse eingeben."
        }
        "Enter a valid SFTP, WebDAV or MTP address" => {
            "Gültige SFTP-, WebDAV- oder MTP-Adresse eingeben"
        }
        "Target" => "Ziel",
        "Browse the NAS to pick a share, then enter credentials if needed." => {
            "NAS-Freigabe auswählen und bei Bedarf anschließend anmelden."
        }
        "Username (optional)" => "Benutzername (optional)",
        "Password (optional)" => "Passwort (optional)",
        "Advanced login options" => "Erweiterte Anmeldeoptionen",
        "Credentials are never stored in the URL. Saving uses your Linux keyring." => {
            "Zugangsdaten stehen nie in der URL. Speichern erfolgt über den Linux-Schlüsselbund."
        }
        "Saved network connections" => "Gespeicherte Netzwerkverbindungen",
        "Other devices" => "Weitere Geräte",
        "SMB, SFTP, WebDAV or MTP address (no passwords in the URL):" => {
            "SMB-, SFTP-, WebDAV- oder MTP-Adresse (kein Passwort in der URL):"
        }
        "Username (leave empty for server default):" => "Benutzername (leer für Servervorgabe):",
        "NAS username" => "NAS-Benutzername",
        "Domain / workgroup (optional):" => "Domäne / Arbeitsgruppe (optional):",
        "Password (leave empty for guest / existing session):" => {
            "Passwort (leer für Gast / bestehende Sitzung):"
        }
        "NAS password" => "NAS-Passwort",
        "Remember login in Linux keyring" => "Zugang im Linux-Schlüsselbund merken",
        "Only after a successful connection. Requires libsecret-tools and an unlocked Secret Service; never stored in RiceField settings." => {
            "Nur nach erfolgreicher Verbindung. Benötigt libsecret-tools und einen entsperrten Schlüsselbund; niemals in RiceField-Einstellungen gespeichert."
        }
        "Use saved login" => "Gespeicherten Zugang verwenden",
        "Forget saved login" => "Gespeicherten Zugang vergessen",
        "Passwords are never included in the URL. Saving is opt-in and uses your desktop's encrypted Secret Service." => {
            "Passwörter gehören nicht in URLs. Optionales Speichern nutzt den verschlüsselten Schlüsselbund der Desktop-Sitzung."
        }
        "Connect" => "Verbinden",
        "Mount" => "Einhängen",
        "Unmount" => "Aushängen",
        "Eject" => "Auswerfen",
        "Unlock encrypted drive" => "Verschlüsselten Datenträger entsperren",
        "Lock encrypted drive" => "Verschlüsselten Datenträger sperren",
        "Power off USB drive" => "USB-Datenträger ausschalten",
        "Upload to SFTP · safe publication" => "SFTP-Upload · sichere Veröffentlichung",
        "Upload to SMB · safe publication" => "SMB-Upload · sichere Veröffentlichung",
        "Upload to WebDAV · safe publication" => "WebDAV-Upload · sichere Veröffentlichung",
        "Destination directory:" => "Zielordner:",
        "Local regular file (absolute path):" => "Lokale reguläre Datei (absoluter Pfad):",
        "SSH username (blank = your SSH default):" => "SSH-Benutzername (leer = SSH-Vorgabe):",
        "SMB username (required):" => "SMB-Benutzername (erforderlich):",
        "Username and password (or leave blank to try a saved login):" => {
            "Benutzername und Passwort (oder leer für gespeicherten Zugang):"
        }
        "SSH username" => "SSH-Benutzername",
        "SMB username" => "SMB-Benutzername",
        "WebDAV username" => "WebDAV-Benutzername",
        "Use a configured SSH key or SSH agent and a trusted known_hosts entry. Password login is unavailable in this safe upload path. Names and paths are restricted to ASCII letters, digits, spaces, dots, dashes and underscores." => {
            "Eingerichteten SSH-Schlüssel oder SSH-Agenten und einen vertrauenswürdigen known_hosts-Eintrag verwenden. Passwortanmeldung ist für diesen sicheren Upload nicht möglich. Für Namen und Pfade sind nur ASCII-Buchstaben, Ziffern, Leerzeichen, Punkte, Bindestriche und Unterstriche erlaubt."
        }
        "Domain (optional)" => "Domäne (optional)",
        "Password" => "Passwort",
        "Hide" => "Verbergen",
        "Show" => "Anzeigen",
        "Allow unencrypted HTTP for this upload" => {
            "Unverschlüsseltes HTTP für diesen Upload erlauben"
        }
        "HTTP may expose credentials and file contents on the network. Prefer davs:// with a trusted certificate." => {
            "HTTP kann Zugangsdaten und Dateiinhalte im Netzwerk offenlegen. davs:// mit vertrauenswürdigem Zertifikat bevorzugen."
        }
        "RiceField probes OpenSSH hardlink collision safety, uploads into a private directory, reads every byte back, and publishes using an exclusive server-side hardlink. The server must support hardlink@openssh.com. Uncertain stages are recorded for manual review." => {
            "RiceField prüft OpenSSH-Hardlinks auf Kollisionssicherheit, lädt in einen privaten Ordner hoch, liest alle Bytes zur Kontrolle zurück und veröffentlicht über einen exklusiven Hardlink. Der Server muss hardlink@openssh.com unterstützen. Unsichere Zwischenstände werden zur manuellen Prüfung protokolliert."
        }
        "RiceField requires SMB2/3, signed client connections and smbclient. It tests server-side no-replace hardlinks with competing clients before uploading a file. It then reads every staged byte back and publishes via a single hardlink. Access details use an ephemeral local pipe, never URL, arguments or disk. Servers lacking this capability remain read-only." => {
            "RiceField benötigt SMB2/3, signierte Verbindungen und smbclient. Vor dem Upload werden serverseitige, kollisionsfreie Hardlinks mit konkurrierenden Clients geprüft. Danach werden alle Bytes zurückgelesen und die Datei über einen Hardlink veröffentlicht. Zugangsdaten werden nur über eine temporäre lokale Pipe übertragen, nie über URL, Argumente oder Datenträger. Server ohne diese Funktion bleiben schreibgeschützt."
        }
        "RiceField probes WebDAV overwrite protection, uploads a private stage and verifies every byte. The server must accept Overwrite:F; existing destinations are not replaced. Uncertain stages are recorded for manual review." => {
            "RiceField prüft den WebDAV-Überschreibschutz, lädt in einen privaten Zwischenstand hoch und kontrolliert jedes Byte. Der Server muss Overwrite:F unterstützen; bestehende Ziele werden nicht ersetzt. Unsichere Zwischenstände werden zur manuellen Prüfung protokolliert."
        }
        "Verify server and upload" => "Server prüfen und hochladen",
        "Discover network computers" => "Netzwerkcomputer finden",
        "Nearby advertised SMB/NAS servers (no automatic connections):" => {
            "Erkannte SMB-/NAS-Server in der Nähe (keine automatische Verbindung):"
        }
        "Scan again" => "Erneut suchen",
        "Looking for network shares…" => "Netzwerkfreigaben werden gesucht…",
        "No NAS was advertised. You can still connect using its address." => {
            "Kein NAS wurde angekündigt. Eine direkte Verbindung über die Adresse ist weiterhin möglich."
        }
        "Browse shares" => "Freigaben durchsuchen",
        "List shares using GVfs; sign in when a share requires it" => {
            "Freigaben über GVfs auflisten; bei Bedarf anmelden"
        }
        "New tab" => "Neuer Tab",
        "Browse this server without replacing the current folder" => {
            "Diesen Server durchsuchen, ohne den aktuellen Ordner zu ersetzen"
        }
        "Sign in" => "Anmelden",
        "File transfer recovery" => "Dateiübertragungen wiederherstellen",
        "Recovery never deletes original files or overwrites destinations. Confirmed actions can publish a verified private stage, discard only an unpublished private stage, or retire a verified publication record. No action runs automatically." => {
            "Die Wiederherstellung löscht niemals Originaldateien und überschreibt keine Ziele. Bestätigte Aktionen können einen geprüften privaten Zwischenstand veröffentlichen, nur unveröffentlichte Zwischenstände verwerfen oder einen bestätigten Veröffentlichungsnachweis abschließen. Keine Aktion startet automatisch."
        }
        "Compare file contents on demand (maximum 512 MiB and 10,000 entries). A match is only a snapshot, never authorization to delete a source." => {
            "Dateiinhalte nur auf Anforderung vergleichen (maximal 512 MiB und 10.000 Einträge). Übereinstimmung ist eine Momentaufnahme und niemals die Erlaubnis, die Quelle zu löschen."
        }
        "You can prepare a new Copy with a full preview. For v1 records without saved source identity, this uses only the current source path after a separate warning. Neither route completes the interrupted Move or clears its journal." => {
            "Eine neue Kopie kann mit vollständiger Vorschau vorbereitet werden. Bei v1-Einträgen ohne gespeicherte Quellidentität wird nach gesonderter Warnung nur der aktuelle Quellpfad verwendet. Keine dieser Aktionen beendet die unterbrochene Verschiebung oder löscht deren Journal."
        }
        "Cannot read all journal records: {error}" => "Nicht alle Journaleinträge lesbar: {error}",
        "Checking operation records…" => "Vorgangsprotokolle werden geprüft…",
        "Cancel verification" => "Prüfung abbrechen",
        "Source: {path}" => "Quelle: {path}",
        "Current source: {status}" => "Aktuelle Quelle: {status}",
        "Source identity: {status}" => "Quellidentität: {status}",
        "Destination: {path}" => "Ziel: {path}",
        "Current destination: {status}" => "Aktuelles Ziel: {status}",
        "Destination identity: {status}" => "Zielidentität: {status}",
        "Staging → publication: {status}" => "Zwischenstand → Veröffentlichung: {status}",
        "Staging: {path}" => "Zwischenstand: {path}",
        "Current staging: {status}" => "Aktueller Zwischenstand: {status}",
        "Browse source folder" => "Quellordner öffnen",
        "Browse destination folder" => "Zielordner öffnen",
        "Browse staging folder" => "Zwischenstandsordner öffnen",
        "Prepare separate Copy (keep both)…" => "Separate Kopie vorbereiten (beide behalten)…",
        "Start a new copy preview only; keep the original source and recovery record. Unavailable if the saved source identity changed or cannot be checked." => {
            "Nur eine neue Kopiervorschau starten; Originalquelle und Wiederherstellungseintrag erhalten. Nicht verfügbar, wenn die gespeicherte Quellidentität geändert wurde oder nicht prüfbar ist."
        }
        "This old journal did not save the original inode. The current source path may contain a DIFFERENT file. Prepare a completely NEW Keep Both copy preview from what exists now? Nothing is deleted or replayed, and this journal remains open." => {
            "Dieses alte Journal enthält nicht die ursprüngliche Inode. Unter dem aktuellen Quellpfad kann eine ANDERE Datei liegen. Eine vollständig NEUE Kopiervorschau mit „Beide behalten“ aus dem heutigen Bestand vorbereiten? Es wird nichts gelöscht oder erneut ausgeführt; das Journal bleibt offen."
        }
        "Confirm new Copy preview" => "Neue Kopiervorschau bestätigen",
        "New Copy from unproven legacy path…" => "Neue Kopie aus ungeprüftem Altpfad…",
        "Copy may be incomplete or may not have started." => {
            "Kopie ist möglicherweise unvollständig oder wurde nicht gestartet."
        }
        "Staged data was verified; final publication may have occurred." => {
            "Zwischengespeicherte Daten wurden geprüft; endgültige Veröffentlichung könnte erfolgt sein."
        }
        "Destination was published; the original is intentionally retained." => {
            "Ziel wurde veröffentlicht; das Original bleibt absichtlich erhalten."
        }
        "Private stage discard was confirmed but interrupted. It cannot be published; verify the original and explicitly continue cleanup." => {
            "Das Verwerfen des privaten Zwischenstands wurde bestätigt, aber unterbrochen. Er darf nicht veröffentlicht werden. Original prüfen und Bereinigung ausdrücklich fortsetzen."
        }
        "Private stage was discarded; original retained." => {
            "Privater Zwischenstand verworfen; Original bleibt erhalten."
        }
        "Verified copy completed; original retained." => {
            "Geprüfte Kopie abgeschlossen; Original bleibt erhalten."
        }
        "Comparing bytes in background…" => "Bytes werden im Hintergrund verglichen…",
        "Cancel comparison" => "Vergleich abbrechen",
        "Compare source and destination" => "Quelle und Ziel vergleichen",
        "Compare source and staged copy" => "Quelle und Zwischenstand vergleichen",
        "Read-only comparison against staged payload:" => {
            "Schreibgeschützter Vergleich mit Zwischenstand:"
        }
        "Read-only comparison against destination:" => "Schreibgeschützter Vergleich mit Ziel:",
        "Re-verify this legacy v2 private stage and save NEW current inode evidence? The old journal cannot prove historical stage provenance. No files are moved, published or deleted. A later publication or discard requires its own confirmation and fresh verification." => {
            "Diesen alten privaten v2-Zwischenstand erneut prüfen und NEUE aktuelle Inode-Nachweise sichern? Das alte Journal kann die Herkunft des früheren Zwischenstands nicht beweisen. Keine Dateien werden verschoben, veröffentlicht oder gelöscht. Eine spätere Veröffentlichung oder Verwerfung erfordert eigene Bestätigung und frische Prüfung."
        }
        "Confirm verify and adopt" => "Prüfen und Übernehmen bestätigen",
        "Verify and adopt legacy private stage…" => {
            "Alten privaten Zwischenstand prüfen und übernehmen…"
        }
        "Publish this verified staged copy to the currently vacant destination? The original remains untouched. The operation will recheck source, stage, nested metadata and target and refuse a collision; an interrupted publication may require another recovery review." => {
            "Diese geprüfte Zwischenkopie am derzeit freien Ziel veröffentlichen? Das Original bleibt unangetastet. Quelle, Zwischenstand, verschachtelte Metadaten und Ziel werden erneut geprüft; Konflikte werden abgelehnt. Nach einer Unterbrechung kann eine weitere Wiederherstellungsprüfung nötig sein."
        }
        "Confirm safe publication" => "Sichere Veröffentlichung bestätigen",
        "Publish verified staged copy…" => "Geprüfte Zwischenkopie veröffentlichen…",
        "Discard this unpublished private staged copy? This permanently removes ONLY the staging payload after a fresh byte/identity check; the original and destination remain untouched. An interruption leaves a durable cleanup record, never an auto-retry." => {
            "Diese unveröffentlichte private Zwischenkopie verwerfen? NUR ihr Inhalt wird nach erneuter Byte- und Identitätsprüfung dauerhaft entfernt. Original und Ziel bleiben unangetastet. Eine Unterbrechung hinterlässt einen dauerhaften Bereinigungseintrag; kein automatischer Neuversuch."
        }
        "Confirm discard private stage" => "Privaten Zwischenstand verwerfen bestätigen",
        "Discard verified private stage…" => "Geprüften privaten Zwischenstand verwerfen…",
        "Confirm retiring this journal record? Both files are kept. A fresh identity and byte comparison will run again; failures leave the record intact." => {
            "Diesen Journaleintrag als abgeschlossen markieren? Beide Dateien bleiben erhalten. Identität und Bytes werden erneut geprüft; bei Fehlern bleibt der Eintrag unverändert."
        }
        "Confirm journal resolution" => "Journalabschluss bestätigen",
        "Resolve verified copy…" => "Geprüfte Kopie abschließen…",
        "Continue removal of the SAME recorded private stage only? Its payload may already be incomplete. RiceField rechecks the original, vacant destination and pinned staging inode, and refuses substitutions. The original is never removed." => {
            "Nur die Entfernung DESSELBEN protokollierten privaten Zwischenstands fortsetzen? Sein Inhalt kann bereits unvollständig sein. RiceField prüft Original, freies Ziel und gebundene Inode erneut und lehnt Vertauschungen ab. Das Original wird nie entfernt."
        }
        "Confirm continue private cleanup" => "Fortsetzung der privaten Bereinigung bestätigen",
        "Continue interrupted stage discard…" => "Unterbrochenes Verwerfen fortsetzen…",
        "Uncertain remote transfers and safety probes" => {
            "Ungewisse Remote-Übertragungen und Sicherheitsprüfungen"
        }
        "A timeout or crash may have left private objects on the remote server. RiceField never queries, retries, moves or deletes them during recovery. Review the named paths before discarding local evidence." => {
            "Nach Zeitüberschreitung oder Absturz können private Objekte auf dem Server verblieben sein. RiceField fragt sie bei der Wiederherstellung nicht ab, versucht sie nicht erneut zu übertragen und verschiebt oder löscht sie nicht. Vor dem Verwerfen lokaler Nachweise die angegebenen Pfade prüfen."
        }
        "Cannot inspect remote evidence: {error}" => {
            "Remote-Nachweise konnten nicht geprüft werden: {error}"
        }
        "Some recovery artifacts are unreadable or incomplete. Valid records remain visible; no remote objects or local evidence were deleted. Upload admission still uses strict journal validation." => {
            "Einige Wiederherstellungsartefakte sind unlesbar oder unvollständig. Gültige Einträge bleiben sichtbar; es wurden weder Serverobjekte noch lokale Nachweise gelöscht. Uploads unterliegen weiterhin der strikten Journalprüfung."
        }
        "Remote upload {id}" => "Remote-Upload {id}",
        "State: UNKNOWN / NOT SERVER-VERIFIED — publication and cleanup may have succeeded or failed" => {
            "Status: UNBEKANNT / NICHT AM SERVER GEPRÜFT – Veröffentlichung und Bereinigung können erfolgreich gewesen oder gescheitert sein"
        }
        "Intended destination:" => "Vorgesehenes Ziel:",
        "Private stage (may or may not exist):" => {
            "Privater Zwischenstand (möglicherweise nicht vorhanden):"
        }
        "The stage is a uniquely named server directory. Inspect its /payload entry before removing anything. SMB directory permissions depend on the NAS share ACLs." => {
            "Der Zwischenstand ist ein Serverordner mit eindeutigem Namen. Vor einer Entfernung den Eintrag /payload prüfen. SMB-Ordnerberechtigungen hängen von den Freigabe-ACLs des NAS ab."
        }
        "Copy stage URI" => "Zwischenstands-URI kopieren",
        "Copy destination URI" => "Ziel-URI kopieren",
        "Inspect both server-side paths independently and retain any ambiguous data. The record does not prove whether publication succeeded." => {
            "Beide Serverpfade unabhängig prüfen und unklare Daten aufbewahren. Der Eintrag beweist keine erfolgreiche Veröffentlichung."
        }
        "Have you independently verified the remote stage and destination, and handled any retained data? Confirming removes ONLY this local journal record, not server files." => {
            "Wurden Zwischenstand und Ziel unabhängig auf dem Server geprüft und verbliebene Daten behandelt? Die Bestätigung entfernt NUR den lokalen Journaleintrag, keine Serverdateien."
        }
        "Confirm discard local record" => "Lokalen Eintrag verwerfen bestätigen",
        "I reviewed the server; discard record…" => "Server geprüft; Eintrag verwerfen…",
        "Remote safety probe {id}" => "Remote-Sicherheitsprüfung {id}",
        "State: PROBE UNCERTAIN — inspect the prefix independently; this is not a user file" => {
            "Status: PRÜFUNG UNGEWISS – Präfix unabhängig prüfen; keine Benutzerdatei"
        }
        "Private safety-probe prefix:" => "Präfix der privaten Sicherheitsprüfung:",
        "The SFTP/SMB probe prefix is a uniquely named directory. Inspect a, b and target-0 through target-3. Never assume the directory was cleaned." => {
            "Der Präfix für SFTP-/SMB-Prüfungen bezeichnet einen eindeutig benannten Ordner. a, b und target-0 bis target-3 prüfen. Nicht annehmen, dass der Ordner bereinigt wurde."
        }
        "Check rounds 0–3: each prefix may have -ROUND-a, -ROUND-b and -ROUND-destination objects. They contain disposable test data, not the selected upload." => {
            "Runden 0–3 prüfen: Je Präfix können -ROUND-a, -ROUND-b und -ROUND-destination vorhanden sein. Diese Objekte enthalten Testdaten, nicht den gewählten Upload."
        }
        "Copy probe prefix" => "Prüfpräfix kopieren",
        "Have you independently checked the named probe objects on the server and handled any leftovers? Confirming removes ONLY this local evidence record, not server files." => {
            "Wurden die genannten Prüfobjekte unabhängig am Server überprüft und Reste behandelt? Die Bestätigung entfernt NUR den lokalen Nachweis, keine Serverdateien."
        }
        "Confirm discard probe record" => "Verwerfen des Prüfprotokolls bestätigen",
        "I reviewed the probe objects; discard record…" => {
            "Prüfobjekte geprüft; Eintrag verwerfen…"
        }
        "Checking private remote evidence…" => "Private Remote-Nachweise werden geprüft…",
        "Rescan records" => "Einträge erneut prüfen",
        "Transfers" => "Übertragungen",
        "Running #{id}: {label}" => "Läuft #{id}: {label}",
        "Cancel running transfer" => "Laufende Übertragung abbrechen",
        "Cancellation stops future work; already completed items are not undone." => {
            "Abbruch verhindert weitere Schritte; bereits abgeschlossene Elemente werden nicht rückgängig gemacht."
        }
        "No transfer is running." => "Keine laufende Übertragung.",
        "Waiting ({count})" => "Wartend ({count})",
        "Only copy requests can wait. Removing a waiting job makes no filesystem changes." => {
            "Nur Kopieraufträge können warten. Entfernen eines wartenden Auftrags verändert keine Dateien."
        }
        "Resume waiting copies" => "Wartende Kopien fortsetzen",
        "Pause waiting copies" => "Wartende Kopien pausieren",
        "Remove" => "Entfernen",
        "Recent results ({count})" => "Letzte Ergebnisse ({count})",
        "Inspect any uncertain result before retrying. Earlier sessions keep read-only per-file results; history never enables Undo or replay." => {
            "Ungewisse Ergebnisse vor einem neuen Versuch prüfen. Frühere Sitzungen zeigen nur schreibgeschützte Einzelergebnisse; die Historie ermöglicht weder Rückgängig noch erneute Ausführung."
        }
        "Completed" => "Abgeschlossen",
        "Cancelled" => "Abgebrochen",
        "Failed" => "Fehlgeschlagen",
        "Undo rename" => "Umbenennen rückgängig",
        "Undo batch rename" => "Stapel-Umbenennung rückgängig",
        "Undo new item" => "Neues Element rückgängig",
        "Review unstarted items" => "Nicht gestartete Elemente prüfen",
        "Not started" => "Nicht gestartet",
        "Done" => "Fertig",
        "Copied — source kept" => "Kopiert – Quelle behalten",
        "Skipped" => "Übersprungen",
        "Inspect" => "Prüfen",
        "Archive browser · 0.5 preview" => "Archiv durchsuchen · Vorschau 0.5",
        "Create archive · 0.5 preview" => "Archiv erstellen · Vorschau 0.5",
        "All paths are checked for traversal, links and special files before extraction." => {
            "Vor dem Entpacken werden alle Pfade auf Traversierung, Links und Spezialdateien geprüft."
        }
        "Password (if encrypted):" => "Passwort (falls verschlüsselt):",
        "Session only · never saved" => "Nur diese Sitzung · nicht gespeichert",
        "Reload list" => "Liste neu laden",
        "Encrypted ZIP and supported 7z/RAR archives may need a password; ZIP filenames can be listed while contents remain locked." => {
            "Verschlüsselte ZIP- und unterstützte 7z-/RAR-Archive benötigen eventuell ein Passwort. ZIP-Dateinamen können aufgelistet werden, obwohl die Inhalte gesperrt bleiben."
        }
        "{count} entries" => "{count} Einträge",
        "Select all" => "Alle auswählen",
        "Select none" => "Auswahl aufheben",
        "folder" => "Ordner",
        "Extract into new folder:" => "In neuen Ordner entpacken:",
        "Parent: {path} · Existing destinations will not be overwritten" => {
            "Übergeordneter Ordner: {path} · Bestehende Ziele werden nicht überschrieben"
        }
        "Verify integrity" => "Integrität prüfen",
        "Extract all" => "Alle entpacken",
        "Unpack every entry into the named new folder; never overwrite an existing folder." => {
            "Alle Einträge in den angegebenen neuen Ordner entpacken; bestehende Ordner niemals überschreiben."
        }
        "Extract {count} selected" => "{count} Ausgewählte entpacken",
        "Reading archive directory…" => "Archivverzeichnis wird gelesen…",
        "Enter a password and select Reload list if this archive is locked." => {
            "Falls das Archiv gesperrt ist, Passwort eingeben und Liste neu laden."
        }
        "{count} selected source(s)" => "{count} ausgewählte Quellen",
        "Destination folder: {path}" => "Zielordner: {path}",
        "Archive name:" => "Archivname:",
        "Format" => "Format",
        "Encrypt ZIP with AES-256" => "ZIP mit AES-256 verschlüsseln",
        "Password:" => "Passwort:",
        "Encryption is available only for ZIP archives." => {
            "Verschlüsselung ist nur für ZIP-Archive verfügbar."
        }
        "Output: {name}.{extension} · will never overwrite" => {
            "Ausgabe: {name}.{extension} · überschreibt niemals bestehende Dateien"
        }
        "Create archive" => "Archiv erstellen",
        "Working · {bytes} processed" => "In Arbeit · {bytes} verarbeitet",
        "Cancelling; waiting for archive cleanup…" => {
            "Abbruch läuft; auf Archivbereinigung warten…"
        }
        "Customize independently of file operations. Saved themes stay private to your user account." => {
            "Design unabhängig von Dateivorgängen anpassen. Gespeicherte Designs bleiben privat im Benutzerkonto."
        }
        "Import TOML path:" => "TOML-Pfad importieren:",
        "Import" => "Importieren",
        "Theme imported into editor; original file unchanged" => {
            "Design in den Editor importiert; Originaldatei unverändert"
        }
        "Theme TOML" => "Design-TOML",
        "Validate" => "Prüfen",
        "Valid TOML · visual controls enabled" => "TOML gültig · visuelle Einstellungen aktiviert",
        "Invalid theme: {error}" => "Ungültiges Design: {error}",
        "TOML changed · validate before saving or visual editing" => {
            "TOML geändert · vor dem Speichern oder visuellen Bearbeiten prüfen"
        }
        "Visual editor" => "Visueller Editor",
        "Accent" => "Akzentfarbe",
        "Background" => "Hintergrund",
        "Chrome" => "Fensterrahmen",
        "Panel" => "Bereich",
        "Text" => "Text",
        "Selection" => "Auswahl",
        "Sidebar" => "Seitenleiste",
        "Border" => "Rahmen",
        "Icon size" => "Symbolgröße",
        "Row height" => "Zeilenhöhe",
        "Sidebar width" => "Seitenleistenbreite",
        "Visual changes written to TOML (comments may be reformatted)" => {
            "Visuelle Änderungen in TOML übernommen (Kommentare können neu formatiert werden)"
        }
        "Validate the TOML to unlock color and layout controls." => {
            "TOML prüfen, um Farben und Layout bearbeiten zu können."
        }
        "Save as:" => "Speichern unter:",
        ".toml (a new private file; no overwrites)" => {
            ".toml (neue private Datei; kein Überschreiben)"
        }
        "Save & use theme" => "Design speichern und verwenden",
        "Shell composer · component layout" => "Shell-Gestaltung · Komponentenlayout",
        "Move actual toolbars between top and bottom; ↑/↓ changes the render order. Navigation is required." => {
            "Werkzeugleisten oben oder unten anordnen; ↑/↓ ändert die Reihenfolge. Die Navigation ist erforderlich."
        }
        "Menu bar" => "Menüleiste",
        "Tabs" => "Tabs",
        "Navigation" => "Navigation",
        "Command bar" => "Befehlsleiste",
        "Top" => "Oben",
        "Bottom" => "Unten",
        "Off" => "Aus",
        "Move earlier" => "Nach vorne verschieben",
        "Move later" => "Nach hinten verschieben",
        "Sidebar & preview" => "Seitenleiste und Vorschau",
        "Left" => "Links",
        "Right" => "Rechts",
        "Preview pane" => "Vorschaubereich",
        "Tasks" => "Aufgaben",
        "Places" => "Orte",
        "Favorites" => "Favoriten",
        "Volumes" => "Datenträger",
        "Collapsible sidebar groups" => "Einklappbare Seitenleistengruppen",
        "Status bar" => "Statusleiste",
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
        assert_eq!(
            language_from_environment(|key| match key {
                "LC_MESSAGES" => Some("de_DE.UTF-8".into()),
                "LANG" => Some("en_US.UTF-8".into()),
                _ => None,
            }),
            Language::German
        );
        assert_eq!(
            language_from_environment(|key| (key == "LANG").then(|| "fr_FR".into())),
            Language::English
        );
        assert_eq!(translate(Language::German, "Cancel"), "Abbrechen");
        assert_eq!(translate(Language::English, "Cancel"), "Cancel");
        assert_eq!(translate(Language::German, "Untranslated"), "Untranslated");
    }

    #[test]
    fn dialog_templates_substitute_values_without_losing_paths() {
        let message = translate(Language::German, "Delete {count} item(s) permanently?");
        assert_eq!(
            message.replace("{count}", "2"),
            "2 Elemente endgültig löschen?"
        );
    }

    #[test]
    fn german_specialty_workflows_keep_critical_warnings_and_templates() {
        for key in [
            "File transfer recovery",
            "Uncertain remote transfers and safety probes",
            "A timeout or crash may have left private objects on the remote server. RiceField never queries, retries, moves or deletes them during recovery. Review the named paths before discarding local evidence.",
            "Cancelling; waiting for archive cleanup…",
            "Encrypted ZIP and supported 7z/RAR archives may need a password; ZIP filenames can be listed while contents remain locked.",
            "Only copy requests can wait. Removing a waiting job makes no filesystem changes.",
            "Save & use theme",
        ] {
            assert_ne!(translate(Language::German, key), key, "{key}");
            assert_eq!(translate(Language::English, key), key);
        }
        assert_eq!(
            translate(Language::German, "Destination folder: {path}")
                .replace("{path}", "/tmp/folder"),
            "Zielordner: /tmp/folder"
        );
    }
}
