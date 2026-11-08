//! Unit conversions typed into the launcher: `10 km in mi`, `3.5 kg to lb`, `72 °F as C`, `2*1.5 h = min`.
//!
//! The input is a quantity, a unit, a separator (`in`, `to`, `as`, `into`, `=`, `->` or `→`) and the target unit. The
//! quantity is a number or any arithmetic [`crate::calc`] answers (`(3+4) ft in m`). Units are matched by name, case
//! folded, longest name first, and a unit's name must not run on from a word before it. The last separator splits the
//! input, so a unit named like one (`5 in in cm`) still reads. Both units must measure the same quantity; temperature
//! converts with its offsets. Nothing is fetched: there are no currencies.
//!
//! The result is set to [`SIGNIFICANT_DIGITS`] significant digits, in scientific notation past the calculator's range,
//! with the target unit's symbol. Non-finite results, unknown units and units of different quantities give `None`.

use crate::calc;

/// Significant digits a converted value is shown with.
pub const SIGNIFICANT_DIGITS: usize = 6;
/// The longest input taken, in bytes, as for the calculator.
const MAX_INPUT: usize = 200;

/// What a unit measures; only units of one quantity convert into each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quantity {
    Length,
    Mass,
    Volume,
    Area,
    Time,
    Data,
    Speed,
    Temperature,
    Angle,
}

/// A unit: its names (lower case), the symbol it is shown with, and how a value in it maps to its quantity's base
/// unit: `base = value · factor + offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Unit {
    pub names: &'static [&'static str],
    pub symbol: &'static str,
    pub quantity: Quantity,
    pub factor: f64,
    pub offset: f64,
}

const fn unit(
    names: &'static [&'static str],
    symbol: &'static str,
    quantity: Quantity,
    factor: f64,
) -> Unit {
    Unit {
        names,
        symbol,
        quantity,
        factor,
        offset: 0.0,
    }
}

use Quantity::*;

/// Every unit, in its quantity's base: metres, kilograms, litres, square metres, seconds, bytes, metres per second,
/// kelvins and radians. US customary volumes.
pub const UNITS: &[Unit] = &[
    unit(
        &[
            "mm",
            "millimeter",
            "millimeters",
            "millimetre",
            "millimetres",
        ],
        "mm",
        Length,
        0.001,
    ),
    unit(
        &[
            "cm",
            "centimeter",
            "centimeters",
            "centimetre",
            "centimetres",
        ],
        "cm",
        Length,
        0.01,
    ),
    unit(
        &["m", "meter", "meters", "metre", "metres"],
        "m",
        Length,
        1.0,
    ),
    unit(
        &["km", "kilometer", "kilometers", "kilometre", "kilometres"],
        "km",
        Length,
        1000.0,
    ),
    unit(&["in", "inch", "inches", "\""], "in", Length, 0.0254),
    unit(&["ft", "foot", "feet", "'"], "ft", Length, 0.3048),
    unit(&["yd", "yard", "yards"], "yd", Length, 0.9144),
    unit(&["mi", "mile", "miles"], "mi", Length, 1609.344),
    unit(
        &["nmi", "nautical mile", "nautical miles"],
        "nmi",
        Length,
        1852.0,
    ),
    unit(&["mg", "milligram", "milligrams"], "mg", Mass, 1e-6),
    unit(&["g", "gram", "grams"], "g", Mass, 0.001),
    unit(
        &["kg", "kilogram", "kilograms", "kilo", "kilos"],
        "kg",
        Mass,
        1.0,
    ),
    unit(
        &["t", "tonne", "tonnes", "metric ton", "metric tons"],
        "t",
        Mass,
        1000.0,
    ),
    unit(&["oz", "ounce", "ounces"], "oz", Mass, 0.028_349_523_125),
    unit(&["lb", "lbs", "pound", "pounds"], "lb", Mass, 0.453_592_37),
    unit(&["st", "stone", "stones"], "st", Mass, 6.350_293_18),
    unit(
        &[
            "ml",
            "milliliter",
            "milliliters",
            "millilitre",
            "millilitres",
        ],
        "mL",
        Volume,
        0.001,
    ),
    unit(
        &[
            "cl",
            "centiliter",
            "centiliters",
            "centilitre",
            "centilitres",
        ],
        "cL",
        Volume,
        0.01,
    ),
    unit(
        &["l", "liter", "liters", "litre", "litres"],
        "L",
        Volume,
        1.0,
    ),
    unit(
        &[
            "m3",
            "m³",
            "cubic meter",
            "cubic meters",
            "cubic metre",
            "cubic metres",
        ],
        "m³",
        Volume,
        1000.0,
    ),
    unit(
        &["tsp", "teaspoon", "teaspoons"],
        "tsp",
        Volume,
        0.004_928_921_593_75,
    ),
    unit(
        &["tbsp", "tablespoon", "tablespoons"],
        "tbsp",
        Volume,
        0.014_786_764_781_25,
    ),
    unit(
        &["fl oz", "floz", "fluid ounce", "fluid ounces"],
        "fl oz",
        Volume,
        0.029_573_529_562_5,
    ),
    unit(&["cup", "cups"], "cup", Volume, 0.236_588_236_5),
    unit(&["pt", "pint", "pints"], "pt", Volume, 0.473_176_473),
    unit(&["qt", "quart", "quarts"], "qt", Volume, 0.946_352_946),
    unit(&["gal", "gallon", "gallons"], "gal", Volume, 3.785_411_784),
    unit(
        &[
            "cm2",
            "cm²",
            "sq cm",
            "square centimeter",
            "square centimeters",
        ],
        "cm²",
        Area,
        1e-4,
    ),
    unit(
        &[
            "m2",
            "m²",
            "sq m",
            "square meter",
            "square meters",
            "square metre",
            "square metres",
        ],
        "m²",
        Area,
        1.0,
    ),
    unit(
        &[
            "km2",
            "km²",
            "sq km",
            "square kilometer",
            "square kilometers",
        ],
        "km²",
        Area,
        1e6,
    ),
    unit(&["ha", "hectare", "hectares"], "ha", Area, 1e4),
    unit(&["ac", "acre", "acres"], "ac", Area, 4_046.856_422_4),
    unit(
        &["ft2", "ft²", "sq ft", "square foot", "square feet"],
        "ft²",
        Area,
        0.092_903_04,
    ),
    unit(
        &["mi2", "mi²", "sq mi", "square mile", "square miles"],
        "mi²",
        Area,
        2_589_988.110_336,
    ),
    unit(&["ms", "millisecond", "milliseconds"], "ms", Time, 0.001),
    unit(&["s", "sec", "secs", "second", "seconds"], "s", Time, 1.0),
    unit(&["min", "mins", "minute", "minutes"], "min", Time, 60.0),
    unit(&["h", "hr", "hrs", "hour", "hours"], "h", Time, 3600.0),
    unit(&["d", "day", "days"], "d", Time, 86_400.0),
    unit(&["wk", "week", "weeks"], "wk", Time, 604_800.0),
    // a Julian year, 365.25 days
    unit(&["yr", "year", "years"], "yr", Time, 31_557_600.0),
    unit(&["bit", "bits"], "bit", Data, 0.125),
    unit(&["b", "byte", "bytes"], "B", Data, 1.0),
    unit(&["kb", "kilobyte", "kilobytes"], "kB", Data, 1e3),
    unit(&["mb", "megabyte", "megabytes"], "MB", Data, 1e6),
    unit(&["gb", "gigabyte", "gigabytes"], "GB", Data, 1e9),
    unit(&["tb", "terabyte", "terabytes"], "TB", Data, 1e12),
    unit(&["kib", "kibibyte", "kibibytes"], "KiB", Data, 1024.0),
    unit(&["mib", "mebibyte", "mebibytes"], "MiB", Data, 1_048_576.0),
    unit(
        &["gib", "gibibyte", "gibibytes"],
        "GiB",
        Data,
        1_073_741_824.0,
    ),
    unit(
        &["tib", "tebibyte", "tebibytes"],
        "TiB",
        Data,
        1_099_511_627_776.0,
    ),
    unit(
        &["m/s", "mps", "meters per second", "metres per second"],
        "m/s",
        Speed,
        1.0,
    ),
    unit(
        &[
            "km/h",
            "kmh",
            "kph",
            "kilometers per hour",
            "kilometres per hour",
        ],
        "km/h",
        Speed,
        1.0 / 3.6,
    ),
    unit(&["mph", "mi/h", "miles per hour"], "mph", Speed, 0.447_04),
    unit(&["kn", "kt", "knot", "knots"], "kn", Speed, 1852.0 / 3600.0),
    unit(&["ft/s", "fps", "feet per second"], "ft/s", Speed, 0.3048),
    Unit {
        names: &["c", "°c", "celsius", "degrees celsius"],
        symbol: "°C",
        quantity: Temperature,
        factor: 1.0,
        offset: 273.15,
    },
    Unit {
        names: &["f", "°f", "fahrenheit", "degrees fahrenheit"],
        symbol: "°F",
        quantity: Temperature,
        factor: 5.0 / 9.0,
        offset: 459.67 * 5.0 / 9.0,
    },
    unit(&["k", "kelvin", "kelvins"], "K", Temperature, 1.0),
    unit(&["rad", "radian", "radians"], "rad", Angle, 1.0),
    unit(
        &["deg", "°", "degree", "degrees"],
        "°",
        Angle,
        std::f64::consts::PI / 180.0,
    ),
    unit(
        &["turn", "turns", "rev", "revolution", "revolutions"],
        "turn",
        Angle,
        std::f64::consts::TAU,
    ),
];

/// The separators between the quantity and the target unit, each with the spaces it needs around it.
const SEPARATORS: &[&str] = &[" in ", " to ", " as ", " into ", "=", "->", "→"];

/// A conversion the launcher shows: what was asked, the value, and both as text.
#[derive(Clone, Debug, PartialEq)]
pub struct Conversion {
    /// The quantity as asked, with its unit's symbol: `10 km`.
    pub from: String,
    pub value: f64,
    pub unit: &'static Unit,
    /// The value with the target unit's symbol: `6.21371 mi`.
    pub text: String,
}

/// The unit named `name` exactly (case folded, spaces around it ignored).
pub fn find(name: &str) -> Option<&'static Unit> {
    let name = name.trim().to_lowercase();
    UNITS.iter().find(|u| u.names.contains(&name.as_str()))
}

/// The unit `text` ends with and what comes before it: the longest name that `text` ends with and that does not run on
/// from a letter before it.
fn split_unit(text: &str) -> Option<(&str, &'static Unit)> {
    let folded = text.to_lowercase();
    // lower-casing keeps the byte length of everything a unit name matches against
    if folded.len() != text.len() {
        return None;
    }
    let mut best: Option<(usize, &'static Unit)> = None;
    for unit in UNITS {
        for name in unit.names {
            if !folded.ends_with(name) || best.is_some_and(|(len, _)| len >= name.len()) {
                continue;
            }
            let rest = &folded[..folded.len() - name.len()];
            let runs_on =
                |c: char| c.is_alphabetic() && name.starts_with(|n: char| n.is_alphabetic());
            if rest.chars().next_back().is_some_and(runs_on) {
                continue;
            }
            best = Some((name.len(), unit));
        }
    }
    best.map(|(len, unit)| (&text[..text.len() - len], unit))
}

/// The input split at its last separator: the quantity with its unit, and the target unit.
fn split(input: &str) -> Option<(&str, &str)> {
    let folded = input.to_lowercase();
    if folded.len() != input.len() {
        // a character whose case folding changes its length: no unit has one, so the input cannot be a conversion
        // unless only its target does; give up rather than misplace the split
        return None;
    }
    SEPARATORS
        .iter()
        .filter_map(|sep| folded.rfind(sep).map(|at| (at, sep.len())))
        .max_by_key(|&(at, _)| at)
        .map(|(at, len)| (&input[..at], &input[at + len..]))
}

/// Convert `input`; `None` when it is not a conversion between two known units of one quantity.
pub fn convert(input: &str) -> Option<Conversion> {
    let input = input.trim().trim_start_matches('=').trim();
    if input.is_empty() || input.len() > MAX_INPUT {
        return None;
    }
    let (left, target) = split(input)?;
    let to = find(target)?;
    let (amount, from) = split_unit(left.trim_end())?;
    if from.quantity != to.quantity {
        return None;
    }
    let amount = amount.trim();
    if amount.is_empty() {
        return None;
    }
    let quantity = calc::number(amount)?;
    let base = quantity * from.factor + from.offset;
    let value = (base - to.offset) / to.factor;
    if !value.is_finite() {
        return None;
    }
    Some(Conversion {
        from: format!("{} {}", calc::format_number(quantity), from.symbol),
        value,
        unit: to,
        text: format!(
            "{} {}",
            format_significant(value, SIGNIFICANT_DIGITS),
            to.symbol
        ),
    })
}

/// `v` to `digits` significant digits, without trailing zeros, in the calculator's scientific notation past its range.
pub fn format_significant(v: f64, digits: usize) -> String {
    if v == 0.0 || !v.is_finite() {
        return calc::format_number(v);
    }
    let magnitude = v.abs().log10().floor() as i32;
    let decimals = (digits as i32 - 1 - magnitude).clamp(0, 20) as usize;
    let rounded: f64 = format!("{v:.decimals$}").parse().unwrap_or(v);
    if !(1e-6..1e15).contains(&rounded.abs()) {
        let s = format!("{rounded:.prec$e}", prec = digits.saturating_sub(1));
        let (mantissa, exponent) = s.split_once('e').unwrap_or((&s, "0"));
        let mantissa = if mantissa.contains('.') {
            mantissa.trim_end_matches('0').trim_end_matches('.')
        } else {
            mantissa
        };
        return format!("{mantissa}e{exponent}");
    }
    let s = format!("{rounded:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    };
    if s == "-0" {
        "0".into()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(input: &str) -> String {
        convert(input)
            .unwrap_or_else(|| panic!("{input:?} should convert"))
            .text
    }

    fn value(input: &str) -> f64 {
        convert(input)
            .unwrap_or_else(|| panic!("{input:?} should convert"))
            .value
    }

    #[test]
    fn lengths_masses_and_volumes_convert_through_their_base() {
        assert_eq!(text("10 km in mi"), "6.21371 mi");
        assert_eq!(text("1 mi to km"), "1.60934 km");
        assert_eq!(text("12 in to ft"), "1 ft");
        assert_eq!(text("1 kg in lb"), "2.20462 lb");
        assert_eq!(text("16 oz to lb"), "1 lb");
        assert_eq!(text("1 gal to l"), "3.78541 L");
        assert_eq!(text("250 ml as cups"), "1.05669 cup");
        assert_eq!(text("8 fl oz to ml"), "236.588 mL");
    }

    #[test]
    fn time_data_area_speed_and_angles_convert() {
        assert_eq!(text("90 min in h"), "1.5 h");
        assert_eq!(text("1 day to seconds"), "86400 s");
        assert_eq!(text("1 gib in mb"), "1073.74 MB");
        assert_eq!(text("8 bits to bytes"), "1 B");
        assert_eq!(text("1 ha in m2"), "10000 m²");
        assert_eq!(text("100 km/h to mph"), "62.1371 mph");
        assert_eq!(text("180 deg to rad"), "3.14159 rad");
        assert_eq!(text("1 turn in degrees"), "360 °");
    }

    #[test]
    fn temperature_converts_with_its_offsets() {
        assert_eq!(text("100 c to f"), "212 °F");
        assert_eq!(text("32 °F in C"), "0 °C");
        assert_eq!(text("-40 f to c"), "-40 °C");
        assert_eq!(text("0 k to c"), "-273.15 °C");
        assert_eq!(text("72 fahrenheit as celsius"), "22.2222 °C");
    }

    #[test]
    fn the_quantity_may_be_arithmetic_and_the_spacing_is_free() {
        assert_eq!(text("2*1.5 h = min"), "180 min");
        assert_eq!(text("(3+4) ft in m"), "2.1336 m");
        assert_eq!(text("10km to mi"), "6.21371 mi");
        assert_eq!(text("1.5e3 m to km"), "1.5 km");
        assert_eq!(text("  5 KM  IN  MI "), "3.10686 mi");
        assert_eq!(text("1 m -> cm"), "100 cm");
        assert_eq!(text("1 m → mm"), "1000 mm");
        assert_eq!(convert("10 km in mi").unwrap().from, "10 km");
    }

    #[test]
    fn a_unit_named_like_a_separator_still_reads() {
        assert_eq!(text("5 in in cm"), "12.7 cm");
        assert_eq!(text("2 in to mm"), "50.8 mm");
    }

    #[test]
    fn huge_and_tiny_values_go_to_scientific_notation() {
        assert_eq!(text("1e20 km to m"), "1e23 m");
        assert_eq!(text("1 mm to km"), "0.000001 km");
        assert_eq!(text("1 mg in t"), "1e-9 t");
        assert!(value("1e290 tb in bit") > 1e300);
        assert!(
            convert("1e308 tb to bit").is_none(),
            "overflow is no result"
        );
    }

    #[test]
    fn invalid_input_is_no_conversion() {
        for input in [
            "",
            "10 km",
            "km in mi",
            "10 in",
            "10 km in",
            "10 km in kg",
            "10 parsecs in m",
            "10 km in furlongs",
            "1/0 km in m",
            "0/0 m to cm",
            "sqrt(-1) m to cm",
            "ten km in mi",
            "10 skm in mi",
            "10 km in mi in",
            "firefox to chrome",
            "100 c to kg",
            "=",
            "->",
        ] {
            assert!(convert(input).is_none(), "{input:?}");
        }
        assert!(convert(&format!("1 m to {}", "c".repeat(300))).is_none());
    }

    #[test]
    fn unit_names_do_not_run_on_from_a_word() {
        // "5 thing" does not end in grams, "10 cm" ends in centimetres and not metres
        assert!(convert("5 thing to kg").is_none());
        assert_eq!(text("10 cm to m"), "0.1 m");
    }

    #[test]
    fn garbage_never_panics() {
        for input in [
            "é in m",
            "ß to ss",
            "10 İ in m",
            "🙂 km in mi",
            "1 m to 🙂",
            "\" to '",
            "in in in",
        ] {
            let _ = convert(input);
        }
    }

    #[test]
    fn significant_digits() {
        assert_eq!(format_significant(6.213_711_922, 6), "6.21371");
        assert_eq!(format_significant(1234.5678, 6), "1234.57");
        assert_eq!(format_significant(0.000_123_456_78, 6), "0.000123457");
        assert_eq!(format_significant(-0.0, 6), "0");
        assert_eq!(format_significant(999_999.7, 6), "1000000");
        assert_eq!(format_significant(2.5e22, 6), "2.5e22");
    }
}
