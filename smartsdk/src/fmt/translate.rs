//! System-wide translation registry for Smart OS — Phase 44.

use crate::fmt::locale::Locale;

/// Lookup a translation for a given key in the specified locale.
pub fn translate<'a>(key: &'a str, locale: Locale) -> &'a str {
    match locale {
        Locale::FrFR => match key {
            "OK" => "OK",
            "Cancel" => "Annuler",
            "Files" => "Fichiers",
            "Settings" => "Paramètres",
            "Search" => "Rechercher",
            _ => key,
        },
        Locale::EsES => match key {
            "OK" => "Aceptar",
            "Cancel" => "Cancelar",
            "Files" => "Archivos",
            "Settings" => "Ajustes",
            "Search" => "Buscar",
            _ => key,
        },
        Locale::DeDE => match key {
            "OK" => "OK",
            "Cancel" => "Abbrechen",
            "Files" => "Dateien",
            "Settings" => "Einstellungen",
            "Search" => "Suchen",
            _ => key,
        },
        _ => key, // Default to English
    }
}

/// A macro for easy translation lookups.
#[macro_export]
macro_rules! tr {
    ($key:expr) => {
        $crate::fmt::translate::translate($key, $crate::fmt::locale::Locale::current())
    };
}
