//! Implementation of font shaping for Arabic scripts
//!
//! Code herein follows the specification at:
//! <https://github.com/n8willis/opentype-shaping-documents/blob/master/opentype-shaping-arabic-general.md>

use crate::error::{ParseError, ShapingError};
use crate::gsub::{self, FeatureMask, GlyphData, GlyphOrigin, RawGlyph};
use crate::layout::{GDEFTable, LayoutCache, LayoutTable, GSUB};
use crate::tag;

use std::convert::From;
use unicode_joining_type::{get_joining_type, JoiningType};

#[derive(Clone)]
struct ArabicData {
    joining_type: JoiningType,
    canonical_combining_class: u8,
    feature_tag: u32,
}

impl GlyphData for ArabicData {
    fn merge(data1: ArabicData, _data2: ArabicData) -> ArabicData {
        // TODO hold off for future Unicode normalisation changes
        data1
    }
}

// Arabic glyphs are represented as `RawGlyph` structs with `ArabicData` for its `extra_data`.
type ArabicGlyph = RawGlyph<ArabicData>;

impl ArabicGlyph {
    fn is_transparent(&self) -> bool {
        self.extra_data.joining_type == JoiningType::Transparent || self.multi_subst_dup
    }

    fn is_left_joining(&self) -> bool {
        self.extra_data.joining_type == JoiningType::LeftJoining
            || self.extra_data.joining_type == JoiningType::DualJoining
            || self.extra_data.joining_type == JoiningType::JoinCausing
    }

    fn is_right_joining(&self) -> bool {
        self.extra_data.joining_type == JoiningType::RightJoining
            || self.extra_data.joining_type == JoiningType::DualJoining
            || self.extra_data.joining_type == JoiningType::JoinCausing
    }

    fn canonical_combining_class(&self) -> u8 {
        self.extra_data.canonical_combining_class
    }

    fn feature_tag(&self) -> u32 {
        self.extra_data.feature_tag
    }

    fn set_feature_tag(&mut self, feature_tag: u32) {
        self.extra_data.feature_tag = feature_tag
    }
}

impl From<&RawGlyph<()>> for ArabicGlyph {
    fn from(raw_glyph: &RawGlyph<()>) -> ArabicGlyph {
        // Since there's no `Char` to work out the `ArabicGlyph`s joining type when the glyph's
        // `glyph_origin` is `GlyphOrigin::Direct`, we fallback to `JoiningType::NonJoining` as
        // the safest approach
        let joining_type = match raw_glyph.glyph_origin {
            GlyphOrigin::Char(c) => get_joining_type(c),
            GlyphOrigin::Direct => JoiningType::NonJoining,
        };

        let canonical_combining_class = match raw_glyph.glyph_origin {
            GlyphOrigin::Char(c) => canonical_combining_class(c),
            GlyphOrigin::Direct => 0,
        };

        ArabicGlyph {
            unicodes: raw_glyph.unicodes.clone(),
            glyph_index: raw_glyph.glyph_index,
            liga_component_pos: raw_glyph.liga_component_pos,
            glyph_origin: raw_glyph.glyph_origin,
            small_caps: raw_glyph.small_caps,
            multi_subst_dup: raw_glyph.multi_subst_dup,
            is_vert_alt: raw_glyph.is_vert_alt,
            fake_bold: raw_glyph.fake_bold,
            fake_italic: raw_glyph.fake_italic,
            variation: raw_glyph.variation,
            extra_data: ArabicData {
                joining_type,
                canonical_combining_class,
                // For convenience, we loosely follow the spec (`2. Computing letter joining
                // states`) here by initialising all `ArabicGlyph`s to `tag::ISOL`
                feature_tag: tag::ISOL,
            },
        }
    }
}

impl From<&ArabicGlyph> for RawGlyph<()> {
    fn from(arabic_glyph: &ArabicGlyph) -> RawGlyph<()> {
        RawGlyph {
            unicodes: arabic_glyph.unicodes.clone(),
            glyph_index: arabic_glyph.glyph_index,
            liga_component_pos: arabic_glyph.liga_component_pos,
            glyph_origin: arabic_glyph.glyph_origin,
            small_caps: arabic_glyph.small_caps,
            multi_subst_dup: arabic_glyph.multi_subst_dup,
            is_vert_alt: arabic_glyph.is_vert_alt,
            fake_bold: arabic_glyph.fake_bold,
            variation: arabic_glyph.variation,
            fake_italic: arabic_glyph.fake_italic,
            extra_data: (),
        }
    }
}

pub fn gsub_apply_arabic(
    gsub_cache: &LayoutCache<GSUB>,
    gsub_table: &LayoutTable<GSUB>,
    gdef_table: Option<&GDEFTable>,
    script_tag: u32,
    lang_tag: Option<u32>,
    raw_glyphs: &mut Vec<RawGlyph<()>>,
) -> Result<(), ShapingError> {
    match gsub_table.find_script(script_tag)? {
        Some(s) => {
            if s.find_langsys_or_default(lang_tag)?.is_none() {
                return Ok(());
            }
        }
        None => return Ok(()),
    }

    let arabic_glyphs = &mut raw_glyphs.iter().map(ArabicGlyph::from).collect();

    // 1. Compound character composition and decomposition

    apply_lookups(
        FeatureMask::CCMP,
        gsub_cache,
        gsub_table,
        gdef_table,
        script_tag,
        lang_tag,
        arabic_glyphs,
        |_, _| true,
    )?;

    // 2. Computing letter joining states

    {
        let mut previous_i = arabic_glyphs
            .iter()
            .position(|g| !g.is_transparent())
            .unwrap_or(0);

        for i in (previous_i + 1)..arabic_glyphs.len() {
            if arabic_glyphs[i].is_transparent() {
                continue;
            }

            if arabic_glyphs[previous_i].is_left_joining() && arabic_glyphs[i].is_right_joining() {
                arabic_glyphs[i].set_feature_tag(tag::FINA);

                match arabic_glyphs[previous_i].feature_tag() {
                    tag::ISOL => arabic_glyphs[previous_i].set_feature_tag(tag::INIT),
                    tag::FINA => arabic_glyphs[previous_i].set_feature_tag(tag::MEDI),
                    _ => {}
                }
            }

            previous_i = i;
        }
    }

    // 3. Applying the stch feature
    //
    // TODO hold off for future generalised solution (including the Syriac Abbreviation Mark)

    // 4. Applying the language-form substitution features from GSUB

    const LANGUAGE_FEATURES: &'static [(FeatureMask, bool)] = &[
        (FeatureMask::LOCL, true),
        (FeatureMask::ISOL, false),
        (FeatureMask::FINA, false),
        (FeatureMask::MEDI, false),
        (FeatureMask::INIT, false),
        (FeatureMask::RLIG, true),
        (FeatureMask::RCLT, true),
        (FeatureMask::CALT, true),
    ];

    for &(feature_mask, is_global) in LANGUAGE_FEATURES {
        apply_lookups(
            feature_mask,
            gsub_cache,
            gsub_table,
            gdef_table,
            script_tag,
            lang_tag,
            arabic_glyphs,
            |g, feature_tag| is_global || g.feature_tag() == feature_tag,
        )?;
    }

    // 5. Applying the typographic-form substitution features from GSUB
    //
    // Note that we skip `GSUB`'s `DLIG` and `CSWH` features as results would differ from other
    // Arabic shapers

    const TYPOGRAPHIC_FEATURES: &'static [FeatureMask] = &[FeatureMask::LIGA, FeatureMask::MSET];

    for &feature_mask in TYPOGRAPHIC_FEATURES {
        apply_lookups(
            feature_mask,
            gsub_cache,
            gsub_table,
            gdef_table,
            script_tag,
            lang_tag,
            arabic_glyphs,
            |_, _| true,
        )?;
    }

    // 6. Mark reordering
    //
    // This is currently not implemented as results would then differ from other Arabic shapers

    *raw_glyphs = arabic_glyphs.iter().map(RawGlyph::from).collect();

    Ok(())
}

fn apply_lookups(
    feature_mask: FeatureMask,
    gsub_cache: &LayoutCache<GSUB>,
    gsub_table: &LayoutTable<GSUB>,
    gdef_table: Option<&GDEFTable>,
    script_tag: u32,
    lang_tag: Option<u32>,
    arabic_glyphs: &mut Vec<RawGlyph<ArabicData>>,
    pred: impl Fn(&RawGlyph<ArabicData>, u32) -> bool + Copy,
) -> Result<(), ParseError> {
    let index = gsub::get_lookups_cache_index(gsub_cache, script_tag, lang_tag, feature_mask)?;
    let lookups = &gsub_cache.cached_lookups.borrow()[index];

    for &(lookup_index, feature_tag) in lookups {
        gsub::gsub_apply_lookup(
            gsub_cache,
            gsub_table,
            gdef_table,
            lookup_index,
            feature_tag,
            None,
            arabic_glyphs,
            0,
            arabic_glyphs.len(),
            |g| pred(g, feature_tag),
        )?;
    }

    Ok(())
}

fn canonical_combining_class(ch: char) -> u8 {
    match ch {
        '\u{064B}' => 27,  // Fathatan
        '\u{08F0}' => 27,  // Open Fathatan
        '\u{064C}' => 28,  // Dammatan
        '\u{08F1}' => 28,  // Open Dammatan
        '\u{064D}' => 29,  // Kasratan
        '\u{08F2}' => 29,  // Open Kasratan
        '\u{0618}' => 30,  // Small Fatha
        '\u{064E}' => 30,  // Fatha
        '\u{0619}' => 31,  // Small Damma
        '\u{064F}' => 31,  // Damma
        '\u{061A}' => 32,  // Small Kasra
        '\u{0650}' => 32,  // Kasra
        '\u{0651}' => 33,  // Shadda
        '\u{0652}' => 34,  // Sukun
        '\u{0670}' => 35,  // Letter Superscript Alef
        '\u{0655}' => 220, // Hamza Below
        '\u{0656}' => 220, // Subscript Alef
        '\u{065C}' => 220, // Vowel Sign Dot Below
        '\u{065F}' => 220, // Wavy Hamza Below
        '\u{06E3}' => 220, // Small Low Seen
        '\u{06EA}' => 220, // Empty Centre Low Stop
        '\u{06ED}' => 220, // Small Low Meem
        '\u{08D3}' => 220, // Small Low Waw
        '\u{08E3}' => 220, // Turned Damma Below
        '\u{08E6}' => 220, // Curly Kasra
        '\u{08E9}' => 220, // Curly Kasratan
        '\u{08ED}' => 220, // Tone One Dot Below
        '\u{08EE}' => 220, // Tone Two Dots Below
        '\u{08EF}' => 220, // Tone Loop Below
        '\u{08F6}' => 220, // Kasra With Dot Below
        '\u{08F9}' => 220, // Left Arrowhead Below
        '\u{08FA}' => 220, // Right Arrowhead Below
        '\u{0610}' => 230, // Sign Sallallahou Alayhe Wassallam
        '\u{0611}' => 230, // Sign Alayhe Assallam
        '\u{0612}' => 230, // Sign Rahmatullah Alayhe
        '\u{0613}' => 230, // Sign Radi Allahou Anhu
        '\u{0614}' => 230, // Sign Takhallus
        '\u{0615}' => 230, // Small High Tah
        '\u{0616}' => 230, // Small High Ligature Alef With Lam With Yeh
        '\u{0617}' => 230, // Small High Zain
        '\u{0653}' => 230, // Maddah Above
        '\u{0654}' => 230, // Hamza Above
        '\u{0657}' => 230, // Inverted Damma
        '\u{0658}' => 230, // Mark Noon Ghunna
        '\u{0659}' => 230, // Zwarakay
        '\u{065A}' => 230, // Vowel Sign Small V Above
        '\u{065B}' => 230, // Vowel Sign Inverted Small V Above
        '\u{065D}' => 230, // Reversed Damma
        '\u{065E}' => 230, // Fatha With Two Dots
        '\u{06D6}' => 230, // Small High Ligature Sad With Lam With Alef Maksura
        '\u{06D7}' => 230, // Small High Ligature Qaf With Lam With Alef Maksura
        '\u{06D8}' => 230, // Small High Meem Initial Form
        '\u{06D9}' => 230, // Small High Lam Alef
        '\u{06DA}' => 230, // Small High Jeem
        '\u{06DB}' => 230, // Small High Three Dots
        '\u{06DC}' => 230, // Small High Seen
        '\u{06DF}' => 230, // Small High Rounded Zero
        '\u{06E0}' => 230, // Small High Upright Rectangular Zero
        '\u{06E1}' => 230, // Small High Dotless Head Of Khah
        '\u{06E2}' => 230, // Small High Meem Isolated Form
        '\u{06E4}' => 230, // Small High Madda
        '\u{06E7}' => 230, // Small High Yeh
        '\u{06E8}' => 230, // Small High Noon
        '\u{06EB}' => 230, // Empty Centre High Stop
        '\u{06EC}' => 230, // Rounded High Stop With Filled Centre
        '\u{08D4}' => 230, // Small High Word Ar-Rub
        '\u{08D5}' => 230, // Small High Sad
        '\u{08D6}' => 230, // Small High Ain
        '\u{08D7}' => 230, // Small High Qaf
        '\u{08D8}' => 230, // Small High Noon With Kasra
        '\u{08D9}' => 230, // Small Low Noon With Kasra
        '\u{08DA}' => 230, // Small High Word Ath-Thalatha
        '\u{08DB}' => 230, // Small High Word As-Sajda
        '\u{08DC}' => 230, // Small High Word An-Nisf
        '\u{08DD}' => 230, // Small High Word Sakta
        '\u{08DE}' => 230, // Small High Word Qif
        '\u{08DF}' => 230, // Small High Word Waqfa
        '\u{08E0}' => 230, // Small High Footnote Marker
        '\u{08E1}' => 230, // Small High Sign Safha
        '\u{08E4}' => 230, // Curly Fatha
        '\u{08E5}' => 230, // Curly Damma
        '\u{08E7}' => 230, // Curly Fathatan
        '\u{08E8}' => 230, // Curly Dammatan
        '\u{08EA}' => 230, // Tone One Dot Above
        '\u{08EB}' => 230, // Tone Two Dots Above
        '\u{08EC}' => 230, // Tone Loop Above
        '\u{08F3}' => 230, // Small High Waw
        '\u{08F4}' => 230, // Fatha With Ring
        '\u{08F5}' => 230, // Fatha With Dot Above
        '\u{08F7}' => 230, // Left Arrowhead Above
        '\u{08F8}' => 230, // Right Arrowhead Above
        '\u{08FB}' => 230, // Double Right Arrowhead Above
        '\u{08FC}' => 230, // Double Right Arrowhead Above With Dot
        '\u{08FD}' => 230, // Right Arrowhead Above With Dot
        '\u{08FE}' => 230, // Damma With Dot
        '\u{08FF}' => 230, // Mark Sideways Noon Ghunna
        _ => 0,
    }
}
