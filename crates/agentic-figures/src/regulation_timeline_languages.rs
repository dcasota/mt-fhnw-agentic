//! Per-language UI string table for the regulation-timeline figure.
//!
//! Port of the python kit's `LANGUAGES` dict (lines 562-925 of
//! `_render_regulation_timeline_v3.py` v_130732). One block per
//! supported language; each block carries every string the renderer
//! can emit (panel titles, axis labels, cheat-sheet, jurisdiction
//! names, goal names).
//!
//! **Swiss German note:** the kit's `de` block already follows Swiss
//! Standard German orthography (`Strasse`, `Bussgelder`, `Strafmass`)
//! — see memory `swiss-standard-german-required.md`. No additional
//! transformation needed.

#![allow(dead_code)] // Renderers in follow-up commits.

/// `LANGUAGES[i] = (lang_tag, &[(key, value)])`.
///
/// Keys are stable; values are the per-language string. Use the
/// [`t`] helper for lookup with English fallback.
pub const LANGUAGES: &[(&str, &[(&str, &str)])] = &[
    ("en", EN),
    ("de", DE),
    ("fr", FR),
    ("it", IT),
    ("rm", RM),
    ("hi", HI),
];

/// Translate a UI key into `lang`, falling back to English and then
/// to the raw key (mirrors the python `t(key, lang)` helper).
#[must_use]
pub fn t<'a>(lang: &str, key: &'a str) -> &'a str {
    let lookup = |code: &str| {
        LANGUAGES
            .iter()
            .find(|(l, _)| *l == code)
            .map(|(_, kv)| *kv)
    };
    let prefer = lookup(lang).or_else(|| lookup("en"));
    if let Some(table) = prefer
        && let Some(v) = table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    {
        return v;
    }
    if let Some(en) = lookup("en")
        && let Some(v) = en.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    {
        return v;
    }
    key
}

/// Keys every language block must define — used by the
/// `every_language_defines_every_key` test to lock the schema.
pub const REQUIRED_KEYS: &[&str] = &[
    "suptitle",
    "panel_a_title",
    "panel_b_title",
    "panel_c_title",
    "panel_d_title",
    "year",
    "today",
    "regs_entering",
    "col_sector",
    "col_reach",
    "col_teeth",
    "col_cadence",
    "col_chapters",
    "col_summary",
    "reach_et",
    "hot_flag",
    "cadence_ann",
    "cadence_rev",
    "cadence_stat",
    "col_hot",
    "legend_fadein",
    "legend_inforce",
    "legend_milestone",
    "legend_hotspot",
    "legend_conflict",
    "legend_mutual",
    "cheat_sheet",
    "span_jur_word",
    "jur_EU",
    "jur_US",
    "jur_DE",
    "jur_FR",
    "jur_CH",
    "jur_IN",
    "jur_Intl",
    "jur_Global",
    "goal_data_protection",
    "goal_cybersecurity",
    "goal_operational_resilience",
    "goal_critical_infra",
    "goal_sovereign_cloud",
    "goal_ai_governance",
    "goal_crypto_module",
    "goal_pqc_migration",
    "goal_tls_cert_lifetime",
    "goal_classified_info",
    "goal_qkd_quantum_channels",
    "goal_methodology",
];

// ===========================================================================
// English (canonical — defines every key the renderer can emit).
// ===========================================================================

const EN: &[(&str, &str)] = &[
    (
        "suptitle",
        "Regulations, standards, and time-bounded deadlines cited in Ch1\u{2013}7 \u{2014} meta-methodology view (density \u{00B7} hot-spots \u{00B7} sector \u{00B7} reach \u{00B7} teeth \u{00B7} cadence \u{00B7} chapter citations \u{00B7} mutual-recognition arcs \u{00B7} conflict points)",
    ),
    (
        "panel_a_title",
        "Panel A \u{2014} Enforcement starts per year (stacked by jurisdiction)",
    ),
    (
        "panel_b_title",
        "Panel B \u{2014} Hot-spots: same regulatory goal, mismatched country timelines (line = enforcement spread; markers = each jurisdiction)",
    ),
    (
        "panel_c_title",
        "Panel C \u{2014} Conflict-involved regulations only: pairs that pull in opposite directions",
    ),
    (
        "panel_d_title",
        "Panel D \u{2014} Per-regulation detail (full inventory) + meta-methodology columns (sector | extraterritorial | enforcement teeth | cadence | \u{00A7}chapters)",
    ),
    ("year", "Year"),
    ("today", "today"),
    ("regs_entering", "Regs entering\nenforcement"),
    ("col_sector", "sector"),
    ("col_reach", "reach"),
    ("col_teeth", "teeth"),
    ("col_cadence", "cadence"),
    ("col_chapters", "chapters"),
    ("col_summary", "one-line summary"),
    ("reach_et", "\u{25B6}ET"),
    ("hot_flag", "  critical"),
    ("cadence_ann", "ann"),
    ("cadence_rev", "rev"),
    ("cadence_stat", "static"),
    ("col_hot", "critical"),
    ("legend_fadein", "Pre-effective (fade-in)"),
    ("legend_inforce", "In force"),
    ("legend_milestone", "Hard deadline / milestone"),
    ("legend_hotspot", "Hot-spot goal (\u{2265}3-yr spread)"),
    ("legend_conflict", "Conflict point (\u{26A1})"),
    ("legend_mutual", "Mutual-recognition arc"),
    (
        "cheat_sheet",
        "Sector codes:  HOR=horizontal \u{00B7} FIN=financial \u{00B7} IND=industrial \u{00B7} EMB=embedded \u{00B7} DEF=defense \u{00B7} GOV=government \u{00B7} RET=retail \u{00B7} RES=research \u{00B7} OSS=open-source\nReach:  \u{25B6}ET = extraterritorial market-of-record reach (catches non-domestic suppliers)\nEnforcement teeth:  \u{25CF}\u{25CF}\u{25CF} hard fines / market access \u{00B7} \u{25CF}\u{25CF}\u{25CB} fines / orders \u{00B7} \u{25CF}\u{25CB}\u{25CB} guidance / voluntary\nCadence:  ann = annual revision \u{00B7} rev = periodic revision \u{00B7} static = stable for years\nChapters:  \u{00A7}N tags identify Ch1-7 sections that ground an argument in this regulation",
    ),
    ("span_jur_word", "jur"),
    ("jur_EU", "European Union"),
    ("jur_US", "United States"),
    ("jur_DE", "Germany"),
    ("jur_FR", "France"),
    ("jur_CH", "Switzerland"),
    ("jur_IN", "India"),
    ("jur_Intl", "International (ISO/ETSI)"),
    ("jur_Global", "Global / cross-jurisdictional"),
    ("goal_data_protection", "Data protection"),
    (
        "goal_cybersecurity",
        "Cybersecurity / supply chain integrity",
    ),
    (
        "goal_operational_resilience",
        "Operational resilience (financial-sector ICT)",
    ),
    ("goal_critical_infra", "Critical-infrastructure baselines"),
    (
        "goal_sovereign_cloud",
        "Sovereign cloud / jurisdictional builds",
    ),
    ("goal_ai_governance", "AI governance & risk management"),
    ("goal_crypto_module", "Cryptographic module validation"),
    ("goal_pqc_migration", "Post-quantum cryptography migration"),
    ("goal_tls_cert_lifetime", "TLS / certificate lifetime"),
    (
        "goal_classified_info",
        "Classified-info handling / federal authorisation",
    ),
    ("goal_qkd_quantum_channels", "QKD / quantum-safe channels"),
    ("goal_methodology", "Systematic-review methodology"),
];

// ===========================================================================
// German (Swiss Standard German / Schweizerhochdeutsch — see memory
// `swiss-standard-german-required.md`; the kit's `de` block already
// uses `Strafmass`, `Bussgelder`, etc., so no ß-to-ss transformation
// is needed for the canonical content).
// ===========================================================================

const DE: &[(&str, &str)] = &[
    (
        "suptitle",
        "Regulierungen, Normen und zeitlich befristete Fristen aus Kap. 1\u{2013}7 \u{2014} Meta-Methodik-Sicht (Dichte \u{00B7} Hot-Spots \u{00B7} Sektor \u{00B7} Reichweite \u{00B7} Sanktionsst\u{00E4}rke \u{00B7} Aktualisierungsrhythmus \u{00B7} Kapitelverweise \u{00B7} Anerkennungsb\u{00F6}gen \u{00B7} Konfliktpunkte)",
    ),
    (
        "panel_a_title",
        "Panel A \u{2014} Beginn der Durchsetzung pro Jahr (gestapelt nach Jurisdiktion)",
    ),
    (
        "panel_b_title",
        "Panel B \u{2014} Hot-Spots: gleiches regulatorisches Ziel, abweichende L\u{00E4}nderfristen (Linie = Spannweite des Inkrafttretens; Marker = einzelne Jurisdiktion)",
    ),
    (
        "panel_c_title",
        "Panel C \u{2014} In Konflikt stehende Regulierungen: Paare mit gegens\u{00E4}tzlicher Wirkung",
    ),
    (
        "panel_d_title",
        "Panel D \u{2014} Detail je Regulierung (vollst\u{00E4}ndige Liste) + Meta-Methodik-Spalten (Sektor | extraterritorial | Sanktionsst\u{00E4}rke | Rhythmus | \u{00A7}Kapitel)",
    ),
    ("year", "Jahr"),
    ("today", "heute"),
    ("regs_entering", "In Kraft tretende\nRegulierungen"),
    ("col_sector", "Sektor"),
    ("col_reach", "Reichweite"),
    ("col_teeth", "Strafmass"),
    ("col_cadence", "Rhythmus"),
    ("col_chapters", "Kapitel"),
    ("col_summary", "Einzeiler-Zusammenfassung"),
    ("reach_et", "\u{25B6}ET"),
    ("hot_flag", "  kritisch"),
    ("cadence_ann", "j\u{00E4}hrl."),
    ("cadence_rev", "period."),
    ("cadence_stat", "stabil"),
    ("col_hot", "kritisch"),
    ("legend_fadein", "Vor Inkrafttreten (Einblende)"),
    ("legend_inforce", "In Kraft"),
    ("legend_milestone", "Harter Stichtag / Meilenstein"),
    (
        "legend_hotspot",
        "Hot-Spot-Ziel (\u{2265}3-Jahre-Spannweite)",
    ),
    ("legend_conflict", "Konfliktpunkt (\u{26A1})"),
    ("legend_mutual", "Anerkennungsbogen"),
    (
        "cheat_sheet",
        "Sektor-Codes:  HOR=horizontal \u{00B7} FIN=Finanz \u{00B7} IND=Industrie \u{00B7} EMB=Embedded \u{00B7} DEF=Verteidigung \u{00B7} GOV=Regierung \u{00B7} RET=Retail \u{00B7} RES=Forschung \u{00B7} OSS=Open-Source\nReichweite:  \u{25B6}ET = extraterritorialer Marktzugang (auch ausl\u{00E4}ndische Anbieter betroffen)\nSanktionsst\u{00E4}rke:  \u{25CF}\u{25CF}\u{25CF} harte Bussgelder / Marktzugang \u{00B7} \u{25CF}\u{25CF}\u{25CB} Bussgelder / Anordnungen \u{00B7} \u{25CF}\u{25CB}\u{25CB} Leitfaden / freiwillig\nRhythmus:  j\u{00E4}hrl. = j\u{00E4}hrliche Revision \u{00B7} period. = periodische Revision \u{00B7} stabil = jahrelang unver\u{00E4}ndert\nKapitel:  \u{00A7}N-Tags zeigen Abschnitte aus Kap. 1\u{2013}7, in denen die Regulierung ein Argument tr\u{00E4}gt",
    ),
    ("span_jur_word", "Jur."),
    ("jur_EU", "Europ\u{00E4}ische Union"),
    ("jur_US", "Vereinigte Staaten"),
    ("jur_DE", "Deutschland"),
    ("jur_FR", "Frankreich"),
    ("jur_CH", "Schweiz"),
    ("jur_IN", "Indien"),
    ("jur_Intl", "International (ISO/ETSI)"),
    ("jur_Global", "Global / jurisdiktions\u{00FC}bergreifend"),
    ("goal_data_protection", "Datenschutz"),
    (
        "goal_cybersecurity",
        "Cybersicherheit / Lieferketten\u{00AD}integrit\u{00E4}t",
    ),
    (
        "goal_operational_resilience",
        "Operative Resilienz (Finanz-ICT)",
    ),
    ("goal_critical_infra", "Kritische-Infrastruktur-Basis"),
    (
        "goal_sovereign_cloud",
        "Souver\u{00E4}ne Cloud / jurisdiktionale Builds",
    ),
    ("goal_ai_governance", "KI-Governance & Risikomanagement"),
    ("goal_crypto_module", "Krypto-Modul-Validierung"),
    ("goal_pqc_migration", "Post-Quanten-Krypto-Migration"),
    ("goal_tls_cert_lifetime", "TLS / Zertifikatslaufzeit"),
    (
        "goal_classified_info",
        "Verschlusssachen / staatliche Zulassung",
    ),
    (
        "goal_qkd_quantum_channels",
        "QKD / quantensichere Kan\u{00E4}le",
    ),
    (
        "goal_methodology",
        "Systematische-\u{00DC}bersicht-Methodik",
    ),
];

// ===========================================================================
// French.
// ===========================================================================

const FR: &[(&str, &str)] = &[
    (
        "suptitle",
        "R\u{00E9}glementations, normes et \u{00E9}ch\u{00E9}ances temporelles cit\u{00E9}es dans les chap. 1\u{2013}7 \u{2014} vue m\u{00E9}ta-m\u{00E9}thodologique (densit\u{00E9} \u{00B7} hot-spots \u{00B7} secteur \u{00B7} port\u{00E9}e \u{00B7} sanctions \u{00B7} cadence \u{00B7} citations chapitres \u{00B7} arcs de reconnaissance mutuelle \u{00B7} points de conflit)",
    ),
    (
        "panel_a_title",
        "Panneau A \u{2014} Entr\u{00E9}es en vigueur par ann\u{00E9}e (empil\u{00E9}es par juridiction)",
    ),
    (
        "panel_b_title",
        "Panneau B \u{2014} Hot-spots : m\u{00EA}me objectif r\u{00E9}glementaire, calendriers nationaux divergents (ligne = \u{00E9}cart d\u{2019}application ; marqueurs = chaque juridiction)",
    ),
    (
        "panel_c_title",
        "Panneau C \u{2014} R\u{00E9}glementations en conflit : paires aux effets oppos\u{00E9}s",
    ),
    (
        "panel_d_title",
        "Panneau D \u{2014} D\u{00E9}tail par r\u{00E9}glementation (inventaire complet) + colonnes m\u{00E9}ta-m\u{00E9}thodologiques (secteur | extraterritorial | sanctions | cadence | \u{00A7}chapitres)",
    ),
    ("year", "Ann\u{00E9}e"),
    ("today", "aujourd\u{2019}hui"),
    ("regs_entering", "R\u{00E9}gl. entrant\nen vigueur"),
    ("col_sector", "secteur"),
    ("col_reach", "port\u{00E9}e"),
    ("col_teeth", "sanctions"),
    ("col_cadence", "cadence"),
    ("col_chapters", "chapitres"),
    ("col_summary", "r\u{00E9}sum\u{00E9} en une ligne"),
    ("reach_et", "\u{25B6}ET"),
    ("hot_flag", "  critique"),
    ("cadence_ann", "ann."),
    ("cadence_rev", "r\u{00E9}vis."),
    ("cadence_stat", "stable"),
    ("col_hot", "critique"),
    (
        "legend_fadein",
        "Pr\u{00E9}-effectif (entr\u{00E9}e en fondu)",
    ),
    ("legend_inforce", "En vigueur"),
    ("legend_milestone", "\u{00C9}ch\u{00E9}ance ferme / jalon"),
    (
        "legend_hotspot",
        "Hot-spot (\u{2265}3 ans d\u{2019}\u{00E9}cart)",
    ),
    ("legend_conflict", "Point de conflit (\u{26A1})"),
    ("legend_mutual", "Reconnaissance mutuelle"),
    (
        "cheat_sheet",
        "Codes de secteur : HOR=horizontal \u{00B7} FIN=finance \u{00B7} IND=industriel \u{00B7} EMB=embarqu\u{00E9} \u{00B7} DEF=d\u{00E9}fense \u{00B7} GOV=gouvernement \u{00B7} RET=retail \u{00B7} RES=recherche \u{00B7} OSS=open-source\nPort\u{00E9}e : \u{25B6}ET = port\u{00E9}e extraterritoriale de march\u{00E9} (concerne aussi les fournisseurs non-domestiques)\nSanctions : \u{25CF}\u{25CF}\u{25CF} amendes lourdes / acc\u{00E8}s march\u{00E9} \u{00B7} \u{25CF}\u{25CF}\u{25CB} amendes / ordres \u{00B7} \u{25CF}\u{25CB}\u{25CB} guide / volontaire\nCadence : ann. = r\u{00E9}vision annuelle \u{00B7} r\u{00E9}vis. = r\u{00E9}vision p\u{00E9}riodique \u{00B7} stable = inchang\u{00E9} pendant des ann\u{00E9}es\nChapitres : \u{00E9}tiquettes \u{00A7}N identifient les sections des chap. 1\u{2013}7 qui fondent un argument sur la r\u{00E9}glementation",
    ),
    ("span_jur_word", "jur."),
    ("jur_EU", "Union europ\u{00E9}enne"),
    ("jur_US", "\u{00C9}tats-Unis"),
    ("jur_DE", "Allemagne"),
    ("jur_FR", "France"),
    ("jur_CH", "Suisse"),
    ("jur_IN", "Inde"),
    ("jur_Intl", "International (ISO/ETSI)"),
    ("jur_Global", "Global / inter-juridictionnel"),
    ("goal_data_protection", "Protection des donn\u{00E9}es"),
    (
        "goal_cybersecurity",
        "Cybers\u{00E9}curit\u{00E9} / int\u{00E9}grit\u{00E9} de la cha\u{00EE}ne d\u{2019}approvisionnement",
    ),
    (
        "goal_operational_resilience",
        "R\u{00E9}silience op\u{00E9}rationnelle (ICT financier)",
    ),
    (
        "goal_critical_infra",
        "R\u{00E9}f\u{00E9}rence des infrastructures critiques",
    ),
    (
        "goal_sovereign_cloud",
        "Cloud souverain / builds juridictionnels",
    ),
    ("goal_ai_governance", "Gouvernance IA & gestion des risques"),
    (
        "goal_crypto_module",
        "Validation des modules cryptographiques",
    ),
    (
        "goal_pqc_migration",
        "Migration cryptographie post-quantique",
    ),
    (
        "goal_tls_cert_lifetime",
        "TLS / dur\u{00E9}e de vie des certificats",
    ),
    (
        "goal_classified_info",
        "Informations classifi\u{00E9}es / autorisation f\u{00E9}d\u{00E9}rale",
    ),
    (
        "goal_qkd_quantum_channels",
        "QKD / canaux quantiquement s\u{00FB}rs",
    ),
    (
        "goal_methodology",
        "M\u{00E9}thodologie des revues syst\u{00E9}matiques",
    ),
];

// ===========================================================================
// Italian.
// ===========================================================================

const IT: &[(&str, &str)] = &[
    (
        "suptitle",
        "Regolamenti, standard e scadenze temporali citati nei capp. 1\u{2013}7 \u{2014} vista meta-metodologica (densit\u{00E0} \u{00B7} hot-spot \u{00B7} settore \u{00B7} portata \u{00B7} sanzioni \u{00B7} cadenza \u{00B7} citazioni capitoli \u{00B7} archi di riconoscimento reciproco \u{00B7} punti di conflitto)",
    ),
    (
        "panel_a_title",
        "Pannello A \u{2014} Entrate in vigore per anno (impilate per giurisdizione)",
    ),
    (
        "panel_b_title",
        "Pannello B \u{2014} Hot-spot: stesso obiettivo regolatorio, scadenze nazionali divergenti (linea = spread di applicazione; marcatori = ogni giurisdizione)",
    ),
    (
        "panel_c_title",
        "Pannello C \u{2014} Regolamenti in conflitto: coppie con effetti opposti",
    ),
    (
        "panel_d_title",
        "Pannello D \u{2014} Dettaglio per regolamento (inventario completo) + colonne meta-metodologiche (settore | extraterritoriale | sanzioni | cadenza | \u{00A7}capitoli)",
    ),
    ("year", "Anno"),
    ("today", "oggi"),
    ("regs_entering", "Reg. in entrata\nin vigore"),
    ("col_sector", "settore"),
    ("col_reach", "portata"),
    ("col_teeth", "sanzioni"),
    ("col_cadence", "cadenza"),
    ("col_chapters", "capitoli"),
    ("col_summary", "riepilogo in una riga"),
    ("reach_et", "\u{25B6}ET"),
    ("hot_flag", "  critico"),
    ("cadence_ann", "ann."),
    ("cadence_rev", "rev."),
    ("cadence_stat", "stabile"),
    ("col_hot", "critico"),
    ("legend_fadein", "Pre-effettivo (dissolvenza)"),
    ("legend_inforce", "In vigore"),
    ("legend_milestone", "Scadenza ferma / milestone"),
    ("legend_hotspot", "Hot-spot (\u{2265}3 anni di spread)"),
    ("legend_conflict", "Punto di conflitto (\u{26A1})"),
    ("legend_mutual", "Riconoscimento reciproco"),
    (
        "cheat_sheet",
        "Codici settore: HOR=orizzontale \u{00B7} FIN=finanza \u{00B7} IND=industriale \u{00B7} EMB=embedded \u{00B7} DEF=difesa \u{00B7} GOV=governativo \u{00B7} RET=retail \u{00B7} RES=ricerca \u{00B7} OSS=open-source\nPortata: \u{25B6}ET = portata extraterritoriale di mercato (coinvolge anche i fornitori non domestici)\nSanzioni: \u{25CF}\u{25CF}\u{25CF} multe pesanti / accesso al mercato \u{00B7} \u{25CF}\u{25CF}\u{25CB} multe / ordini \u{00B7} \u{25CF}\u{25CB}\u{25CB} linee guida / volontario\nCadenza: ann. = revisione annuale \u{00B7} rev. = revisione periodica \u{00B7} stabile = invariato per anni\nCapitoli: tag \u{00A7}N identificano sezioni dei capp. 1\u{2013}7 che fondano un argomento sulla regolamentazione",
    ),
    ("span_jur_word", "giur."),
    ("jur_EU", "Unione Europea"),
    ("jur_US", "Stati Uniti"),
    ("jur_DE", "Germania"),
    ("jur_FR", "Francia"),
    ("jur_CH", "Svizzera"),
    ("jur_IN", "India"),
    ("jur_Intl", "Internazionale (ISO/ETSI)"),
    ("jur_Global", "Globale / inter-giurisdizionale"),
    ("goal_data_protection", "Protezione dei dati"),
    (
        "goal_cybersecurity",
        "Cybersicurezza / integrit\u{00E0} della catena di approvvigionamento",
    ),
    (
        "goal_operational_resilience",
        "Resilienza operativa (ICT settore finanziario)",
    ),
    (
        "goal_critical_infra",
        "Riferimento per infrastrutture critiche",
    ),
    (
        "goal_sovereign_cloud",
        "Cloud sovrano / build giurisdizionali",
    ),
    ("goal_ai_governance", "Governance IA & gestione del rischio"),
    ("goal_crypto_module", "Convalida dei moduli crittografici"),
    (
        "goal_pqc_migration",
        "Migrazione crittografia post-quantistica",
    ),
    ("goal_tls_cert_lifetime", "TLS / durata dei certificati"),
    (
        "goal_classified_info",
        "Informazioni classificate / autorizzazione federale",
    ),
    (
        "goal_qkd_quantum_channels",
        "QKD / canali quantisticamente sicuri",
    ),
    (
        "goal_methodology",
        "Metodologia delle revisioni sistematiche",
    ),
];

// ===========================================================================
// Rumantsch Grischun — the unified Swiss Romansh standard.
// ===========================================================================

const RM: &[(&str, &str)] = &[
    (
        "suptitle",
        "Regulaziuns, normas e termins temporals citads en chaps. 1\u{2013}7 \u{2014} vista metametodologica (densitad \u{00B7} hot-spots \u{00B7} sectur \u{00B7} portada \u{00B7} sancziuns \u{00B7} ritmus \u{00B7} citaziuns chapitels \u{00B7} arcs da renconuschientscha vicendaivla \u{00B7} puncts da conflict)",
    ),
    (
        "panel_a_title",
        "Panel A \u{2014} Entradas en vigur per onn (interconnectadas per giurisdicziun)",
    ),
    (
        "panel_b_title",
        "Panel B \u{2014} Hot-spots: medem ideal regulatoric, termins naziunals divergents (lingia = spann d\u{2019}applicaziun; marcaturas = mintga giurisdicziun)",
    ),
    (
        "panel_c_title",
        "Panel C \u{2014} Regulaziuns en conflict: p\u{00E8}rs cun effects opposits",
    ),
    (
        "panel_d_title",
        "Panel D \u{2014} Detagls per regulaziun (inventari cumplet) + colonnas metametodologicas (sectur | extrateritorial | sancziuns | ritmus | \u{00A7}chapitels)",
    ),
    ("year", "Onn"),
    ("today", "oz"),
    ("regs_entering", "Reg. che entran\nen vigur"),
    ("col_sector", "sectur"),
    ("col_reach", "portada"),
    ("col_teeth", "sancziuns"),
    ("col_cadence", "ritmus"),
    ("col_chapters", "chapitels"),
    ("col_summary", "sumari en ina lingia"),
    ("reach_et", "\u{25B6}ET"),
    ("hot_flag", "  critic"),
    ("cadence_ann", "mintg\u{2019}onn"),
    ("cadence_rev", "reved\u{00EC}"),
    ("cadence_stat", "stabel"),
    ("col_hot", "critic"),
    ("legend_fadein", "Pre-effectiv (slunschiada)"),
    ("legend_inforce", "En vigur"),
    ("legend_milestone", "Termin ferm / etappa"),
    ("legend_hotspot", "Hot-spot (\u{2265}3 onns)"),
    ("legend_conflict", "Punct da conflict (\u{26A1})"),
    ("legend_mutual", "Renconuschientscha vicendaivla"),
    (
        "cheat_sheet",
        "Codas dal sectur: HOR=orizontal \u{00B7} FIN=finanza \u{00B7} IND=industrial \u{00B7} EMB=embedded \u{00B7} DEF=defensiun \u{00B7} GOV=guvern \u{00B7} RET=retail \u{00B7} RES=perscrutaziun \u{00B7} OSS=open-source\nPortada: \u{25B6}ET = portada extrateritoriala dal martg\u{00E0} (lia er purschiders nundomestics)\nSancziuns: \u{25CF}\u{25CF}\u{25CF} multas grevas / access al martg\u{00E0} \u{00B7} \u{25CF}\u{25CF}\u{25CB} multas / ordens \u{00B7} \u{25CF}\u{25CB}\u{25CB} guida / voluntar\nRitmus: mintg\u{2019}onn = revisiun annuala \u{00B7} reved\u{00EC} = revisiun periodica \u{00B7} stabel = invariabel per onns\nChapitels: tags \u{00A7}N inditgeschan secziuns dals chaps. 1\u{2013}7 che basan in argument sin la regulaziun",
    ),
    ("span_jur_word", "giur."),
    ("jur_EU", "Uniun europeica"),
    ("jur_US", "Stadis Unids"),
    ("jur_DE", "Germania"),
    ("jur_FR", "Frantscha"),
    ("jur_CH", "Svizra"),
    ("jur_IN", "India"),
    ("jur_Intl", "Internaziunal (ISO/ETSI)"),
    ("jur_Global", "Global / tranter giurisdicziuns"),
    ("goal_data_protection", "Protecziun da datas"),
    (
        "goal_cybersecurity",
        "Cibersegirezza / integritad da la chadaina d\u{2019}approvisinament",
    ),
    (
        "goal_operational_resilience",
        "Resilienza operativa (ICT dal sectur finanzial)",
    ),
    (
        "goal_critical_infra",
        "Basa d\u{2019}infrastructura criticas",
    ),
    (
        "goal_sovereign_cloud",
        "Cloud suveran / builds giurisdicziunals",
    ),
    ("goal_ai_governance", "Governanza IA & gestiun da ristgs"),
    ("goal_crypto_module", "Validaziun dals moduls criptografics"),
    (
        "goal_pqc_migration",
        "Migraziun da la criptografia post-quanta",
    ),
    ("goal_tls_cert_lifetime", "TLS / durada dals certificats"),
    (
        "goal_classified_info",
        "Infurmaziuns classifitgadas / autorisaziun federala",
    ),
    (
        "goal_qkd_quantum_channels",
        "QKD / chanals segirs cunter il quant",
    ),
    ("goal_methodology", "Metodologia da revisiuns sistematicas"),
];

// ===========================================================================
// Hindi (Devanagari script).
// ===========================================================================

const HI: &[(&str, &str)] = &[
    (
        "suptitle",
        "अध्याय 1\u{2013}7 में उद्धृत विनियम, मानक और समय-सीमाएँ \u{2014} मेटा-कार्यप्रणाली दृश्य (घनत्व \u{00B7} हॉट-स्पॉट \u{00B7} क्षेत्र \u{00B7} पहुँच \u{00B7} दण्ड \u{00B7} अद्यतन ताल \u{00B7} अध्याय उद्धरण \u{00B7} पारस्परिक मान्यता चाप \u{00B7} संघर्ष बिंदु)",
    ),
    (
        "panel_a_title",
        "पैनल A \u{2014} प्रति वर्ष प्रवर्तन आरंभ (अधिकार-क्षेत्र अनुसार स्तरित)",
    ),
    (
        "panel_b_title",
        "पैनल B \u{2014} हॉट-स्पॉट: समान विनियामक लक्ष्य, असंगत देश-कालक्रम (रेखा = प्रवर्तन प्रसार; चिह्न = प्रत्येक अधिकार-क्षेत्र)",
    ),
    (
        "panel_c_title",
        "पैनल C \u{2014} संघर्ष में शामिल विनियम: विपरीत दिशाओं में खींचने वाले जोड़े",
    ),
    (
        "panel_d_title",
        "पैनल D \u{2014} प्रति विनियम विवरण (पूर्ण सूची) + मेटा-कार्यप्रणाली स्तंभ (क्षेत्र | अतिराष्ट्रीय | दण्ड | ताल | \u{00A7}अध्याय)",
    ),
    ("year", "वर्ष"),
    ("today", "आज"),
    ("regs_entering", "प्रवर्तन में\nप्रवेश करते विनियम"),
    ("col_sector", "क्षेत्र"),
    ("col_reach", "पहुँच"),
    ("col_teeth", "दण्ड"),
    ("col_cadence", "ताल"),
    ("col_chapters", "अध्याय"),
    ("col_summary", "एक पंक्ति में सारांश"),
    ("reach_et", "\u{25B6}ET"),
    ("hot_flag", "  गंभीर"),
    ("cadence_ann", "वार्षिक"),
    ("cadence_rev", "संशोधित"),
    ("cadence_stat", "स्थिर"),
    ("col_hot", "गंभीर"),
    ("legend_fadein", "पूर्व-प्रभावी (फेड-इन)"),
    ("legend_inforce", "प्रवर्तन में"),
    ("legend_milestone", "कठोर समय-सीमा / मील का पत्थर"),
    ("legend_hotspot", "हॉट-स्पॉट लक्ष्य (\u{2265}3 वर्ष प्रसार)"),
    ("legend_conflict", "संघर्ष बिंदु (\u{26A1})"),
    ("legend_mutual", "पारस्परिक मान्यता चाप"),
    (
        "cheat_sheet",
        "क्षेत्र कोड: HOR=क्षैतिज \u{00B7} FIN=वित्तीय \u{00B7} IND=औद्योगिक \u{00B7} EMB=एम्बेडेड \u{00B7} DEF=रक्षा \u{00B7} GOV=सरकारी \u{00B7} RET=खुदरा \u{00B7} RES=अनुसंधान \u{00B7} OSS=ओपन-सोर्स\nपहुँच: \u{25B6}ET = बाह्यक्षेत्रीय बाजार पहुँच (गैर-घरेलू आपूर्तिकर्ता भी शामिल)\nदण्ड: \u{25CF}\u{25CF}\u{25CF} कठोर जुर्माना / बाजार पहुँच \u{00B7} \u{25CF}\u{25CF}\u{25CB} जुर्माना / आदेश \u{00B7} \u{25CF}\u{25CB}\u{25CB} मार्गदर्शन / स्वैच्छिक\nताल: वार्षिक = प्रति वर्ष संशोधन \u{00B7} संशोधित = आवधिक संशोधन \u{00B7} स्थिर = वर्षों तक अपरिवर्तित\nअध्याय: \u{00A7}N टैग अध्याय 1\u{2013}7 के अनुभागों की पहचान करते हैं जो विनियम पर तर्क आधारित करते हैं",
    ),
    ("span_jur_word", "अधि."),
    ("jur_EU", "यूरोपीय संघ"),
    ("jur_US", "संयुक्त राज्य अमेरिका"),
    ("jur_DE", "जर्मनी"),
    ("jur_FR", "फ्रांस"),
    ("jur_CH", "स्विट्जरलैंड"),
    ("jur_IN", "भारत"),
    ("jur_Intl", "अंतर्राष्ट्रीय (ISO/ETSI)"),
    ("jur_Global", "वैश्विक / अधिकार-क्षेत्रों में"),
    ("goal_data_protection", "डेटा सुरक्षा"),
    ("goal_cybersecurity", "साइबर सुरक्षा / आपूर्ति शृंखला अखंडता"),
    (
        "goal_operational_resilience",
        "परिचालन लचीलापन (वित्तीय-क्षेत्र ICT)",
    ),
    ("goal_critical_infra", "महत्वपूर्ण-अवसंरचना आधार"),
    ("goal_sovereign_cloud", "संप्रभु क्लाउड / अधिकार-क्षेत्र बिल्ड"),
    ("goal_ai_governance", "AI शासन और जोखिम प्रबंधन"),
    ("goal_crypto_module", "क्रिप्टोग्राफिक मॉड्यूल सत्यापन"),
    ("goal_pqc_migration", "पोस्ट-क्वांटम क्रिप्टोग्राफी प्रवासन"),
    ("goal_tls_cert_lifetime", "TLS / प्रमाणपत्र जीवनकाल"),
    ("goal_classified_info", "वर्गीकृत-सूचना प्रबंधन / संघीय प्राधिकरण"),
    ("goal_qkd_quantum_channels", "QKD / क्वांटम-सुरक्षित चैनल"),
    ("goal_methodology", "व्यवस्थित-समीक्षा कार्यप्रणाली"),
];

// ===========================================================================
// Tests.
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn six_languages_covered() {
        let langs: HashSet<&&str> = LANGUAGES.iter().map(|(l, _)| l).collect();
        for expected in ["en", "de", "fr", "it", "rm", "hi"] {
            assert!(langs.contains(&expected), "LANGUAGES missing '{expected}'");
        }
        assert_eq!(LANGUAGES.len(), 6);
    }

    #[test]
    fn every_language_defines_every_key() {
        for (lang, table) in LANGUAGES {
            let keys: HashSet<&&str> = table.iter().map(|(k, _)| k).collect();
            for req in REQUIRED_KEYS {
                assert!(
                    keys.contains(&req),
                    "language '{lang}' missing required key '{req}'"
                );
            }
        }
    }

    #[test]
    fn no_duplicate_keys_in_any_language() {
        for (lang, table) in LANGUAGES {
            let keys: HashSet<&&str> = table.iter().map(|(k, _)| k).collect();
            assert_eq!(
                keys.len(),
                table.len(),
                "language '{lang}' has a duplicate key"
            );
        }
    }

    #[test]
    fn no_empty_values_in_any_language() {
        for (lang, table) in LANGUAGES {
            for (k, v) in *table {
                assert!(!v.is_empty(), "language '{lang}' key '{k}' has empty value");
            }
        }
    }

    #[test]
    fn german_block_uses_swiss_orthography() {
        // No eszett `ß` anywhere in the DE block (memory:
        // swiss-standard-german-required).
        for (k, v) in DE {
            assert!(
                !v.contains('\u{00DF}'),
                "DE key '{k}' contains eszett `ß`: {v:?} (must use double-s)"
            );
        }
    }

    #[test]
    fn t_returns_english_fallback_for_unknown_lang() {
        // 'ja' is not in LANGUAGES → English fallback.
        let v = t("ja", "year");
        assert_eq!(v, "Year");
    }

    #[test]
    fn t_returns_english_fallback_for_missing_key_in_lang() {
        // Suppose a lang defines 'year' but not 'whatever' — fallback chain
        // should still find 'year' in English.
        let v = t("fr", "year");
        assert_eq!(v, "Ann\u{00E9}e");
        // Unknown key → returns the raw key (last-resort fallback).
        let v = t("en", "definitely_not_a_real_key_zzz");
        assert_eq!(v, "definitely_not_a_real_key_zzz");
    }

    #[test]
    fn t_lookup_works_for_every_language_and_panel_a_title() {
        // Smoke test: every lang must resolve the panel-A title to a
        // non-empty, language-appropriate string.
        for (lang, _) in LANGUAGES {
            let v = t(lang, "panel_a_title");
            assert!(!v.is_empty(), "lang '{lang}' panel_a_title empty");
            assert!(
                v.starts_with("Panel")
                    || v.starts_with("Panneau")
                    || v.starts_with("Pannello")
                    || v.starts_with("\u{092A}"),
                "lang '{lang}' panel_a_title does not start with a recognised panel word: {v:?}"
            );
        }
    }

    #[test]
    fn hindi_panel_titles_start_with_devanagari_panel_word() {
        // `\u{092A}\u{0948}\u{0928}\u{0932}` = "पैनल" (Hindi for "panel").
        let v = t("hi", "panel_a_title");
        assert!(
            v.starts_with("\u{092A}\u{0948}\u{0928}\u{0932}"),
            "Hindi panel_a_title should begin with पैनल: {v:?}"
        );
    }
}
