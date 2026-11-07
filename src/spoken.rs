// Writes out money, dates, years, decimals, ordinals and numbers as words, so the voice never guesses at digits.
// SPDX-License-Identifier: GPL-3.0-or-later
const SMALL: [&str; 20] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
    "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen",
    "nineteen",
];
const TENS: [&str; 10] =
    ["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"];
const MONTHS: [&str; 12] = [
    "january", "february", "march", "april", "may", "june", "july", "august", "september",
    "october", "november", "december",
];

fn under_hundred(n: u64) -> String {
    if n < 20 {
        SMALL[n as usize].into()
    } else if n.is_multiple_of(10) {
        TENS[(n / 10) as usize].into()
    } else {
        format!("{}-{}", TENS[(n / 10) as usize], SMALL[(n % 10) as usize])
    }
}

fn under_thousand(n: u64, uk: bool) -> String {
    let and = if uk { " and" } else { "" };
    match (n / 100, n % 100) {
        (0, r) => under_hundred(r),
        (h, 0) => format!("{} hundred", SMALL[h as usize]),
        (h, r) => format!("{} hundred{and} {}", SMALL[h as usize], under_hundred(r)),
    }
}

pub fn number(n: u64, uk: bool) -> String {
    if n == 0 {
        return "zero".into();
    }
    let groups = [(1_000_000_000_000, "trillion"), (1_000_000_000, "billion"), (1_000_000, "million"), (1000, "thousand")];
    let mut parts = Vec::new();
    let mut rest = n;
    for (size, name) in groups {
        if rest >= size {
            parts.push(format!("{} {name}", number(rest / size, uk)));
            rest %= size;
        }
    }
    if rest > 0 {

        let words = under_thousand(rest, uk);
        if uk && !parts.is_empty() && rest < 100 {
            parts.push(format!("and {words}"));
        } else {
            parts.push(words);
        }
    }
    if uk { parts.join(", ").replace(", and ", " and ") } else { parts.join(" ") }
}

pub fn ordinal(n: u64, uk: bool) -> String {
    let words = number(n, uk);
    let (head, last) = match words.rfind([' ', '-']) {
        Some(i) => (&words[..=i], &words[i + 1..]),
        None => ("", words.as_str()),
    };
    let last = match last {
        "one" => "first".into(),
        "two" => "second".into(),
        "three" => "third".into(),
        "five" => "fifth".into(),
        "eight" => "eighth".into(),
        "nine" => "ninth".into(),
        "twelve" => "twelfth".into(),
        l if l.ends_with('y') => format!("{}ieth", &l[..l.len() - 1]),
        l => format!("{l}th"),
    };
    format!("{head}{last}")
}

pub fn year(n: u64, uk: bool) -> String {
    let (high, low) = (n / 100, n % 100);
    if (2000..2010).contains(&n) || n.is_multiple_of(1000) {
        number(n, uk)
    } else if low == 0 {
        format!("{} hundred", under_hundred(high))
    } else if low < 10 {
        format!("{} oh {}", under_hundred(high), SMALL[low as usize])
    } else {
        format!("{} {}", under_hundred(high), under_hundred(low))
    }
}

const CODE: &str = "\u{1}";

fn digits_said(d: &str) -> String {
    d.chars().map(|c| SMALL[c.to_digit(10).unwrap() as usize]).collect::<Vec<_>>().join(" ")
}

fn is_month(word: &str) -> bool {
    let w = word.trim_matches(|c: char| !c.is_alphabetic()).to_lowercase();
    MONTHS.iter().any(|m| *m == w || (w.len() == 3 && m.starts_with(&w)))
}

fn read_number(chars: &[char], mut i: usize) -> (u64, Option<String>, usize) {
    let mut whole = String::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            whole.push(c);
        } else if c == ',' && chars[i + 1..].iter().take(3).filter(|c| c.is_ascii_digit()).count() == 3
            && chars.get(i + 4).is_none_or(|c| !c.is_ascii_digit())
        {
        } else {
            break;
        }
        i += 1;
    }
    let mut decimals = None;
    if chars.get(i) == Some(&'.') && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
        let mut d = String::new();
        i += 1;
        while let Some(c) = chars.get(i).filter(|c| c.is_ascii_digit()) {
            d.push(*c);
            i += 1;
        }
        decimals = Some(d);
    }
    (whole.parse().unwrap_or(u64::MAX), decimals, i)
}

fn money(whole: u64, pence: Option<&str>, one: &str, many: &str, small: &str, uk: bool) -> String {
    let main = format!("{} {}", number(whole, uk), if whole == 1 { one } else { many });
    match pence.map(|p| format!("{p:0<2}")).and_then(|p| p[..2].parse::<u64>().ok()) {
        Some(0) | None => main,
        Some(p) if whole == 0 => format!("{} {small}", number(p, uk)),

        Some(p) if uk => format!("{main} {}", number(p, uk)),
        Some(p) => format!("{main} and {} {small}", number(p, uk)),
    }
}

fn currency_symbol(c: char) -> Option<(&'static str, &'static str, &'static str)> {
    match c {
        '£' => Some(("pound", "pounds", "pence")),
        '$' => Some(("dollar", "dollars", "cents")),
        '€' => Some(("euro", "euros", "cents")),
        _ => None,
    }
}

fn say_currency(chars: &[char], i: usize, names: (&str, &str, &str), uk: bool) -> (String, usize) {
    let (one, many, small) = names;
    if !chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
        return (format!(" {many} "), i + 1);
    }
    let (whole, decimals, end) = read_number(chars, i + 1);
    let (scale, end) = match chars.get(end) {
        Some('m') if chars.get(end + 1).is_none_or(|c| !c.is_alphabetic()) => (" million", end + 1),
        Some('b') if chars.get(end + 1) == Some(&'n') => (" billion", end + 2),
        Some('k') if chars.get(end + 1).is_none_or(|c| !c.is_alphabetic()) => (" thousand", end + 1),
        _ => ("", end),
    };
    if scale.is_empty() {
        return (money(whole, decimals.as_deref(), one, many, small, uk), end);
    }
    let point = decimals.map(|d| format!(" point {}", digits_said(&d))).unwrap_or_default();
    (format!("{}{point}{scale} {many}", number(whole, uk)), end)
}

fn say_number(chars: &[char], i: usize, out: &mut String, day_before: bool, day_said: &mut bool, uk: bool) -> usize {
    let (whole, decimals, end) = read_number(chars, i);
    if chars[i..end].iter().filter(|c| c.is_ascii_digit()).count() > 18 {
        let all: String = chars[i..end].iter().filter(|c| c.is_ascii_digit()).collect();
        *out += &digits_said(&all);
        return end;
    }
    let after: String = chars[end..].iter().take_while(|c| c.is_alphabetic()).collect();
    let next_word: String = chars[end..].iter().skip_while(|c| *c == &' ').take_while(|c| c.is_alphabetic()).collect();
    let before: String = out.trim_end().rsplit(' ').next().unwrap_or("").to_string();
    let plain_digits = !chars[i..end].contains(&',');
    let mut end = end;
    let said = if let Some(d) = decimals {
        format!("{} point {}", number(whole, uk), digits_said(&d))
    } else if ["st", "nd", "rd", "th"].contains(&after.to_lowercase().as_str()) {
        end += 2;
        ordinal(whole, uk)
    } else if uk && after == "p" {
        end += 1;
        format!("{} {}", number(whole, uk), if whole == 1 { "penny" } else { "pence" })
    } else if (1..=31).contains(&whole) && is_month(&next_word) {
        format!("the {} of", ordinal(whole, uk))
    } else if plain_digits && (1100..=2099).contains(&whole) && end - i == 4 && (is_month(&before) || before.eq_ignore_ascii_case("in") || before.ends_with("of") || is_month(&before.replace(',', "")) || day_before) {
        year(whole, uk)
    } else if plain_digits && (chars[i] == '0' || out.trim_end().ends_with(CODE)) {
        let d = digits_said(&chars[i..end].iter().collect::<String>());
        *out = out.trim_end().trim_end_matches(CODE).to_string() + " ";
        if chars.get(end) == Some(&' ') && chars.get(end + 1).is_some_and(|c| c.is_ascii_digit()) {
            format!("{d}{CODE}")
        } else {
            d
        }
    } else if (1..=31).contains(&whole) && end - i <= 2 && is_month(&before) {
        *day_said = true;
        *out += if uk { " the " } else { " " };
        ordinal(whole, uk)
    } else {
        number(whole, uk)
    };
    *out += &said;
    if chars.get(end).is_some_and(|c| c.is_alphabetic()) {
        out.push(' ');
    }
    end
}

pub fn words(text: &str, uk: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut day_said = false;
    while i < chars.len() {
        let c = chars[i];
        if let Some(names) = currency_symbol(c) {
            let (said, end) = say_currency(&chars, i, names, uk);
            out += &said;
            i = end;
            continue;
        }
        if c.is_ascii_digit() && (i == 0 || !chars[i - 1].is_alphanumeric()) {
            let day_before = std::mem::take(&mut day_said);
            i = say_number(&chars, i, &mut out, day_before, &mut day_said, uk);
            continue;
        }
        match c {
            '%' => out += if uk { " per cent" } else { " percent" },
            '&' => out += " and ",
            c => out.push(c),
        }
        i += 1;
    }
    out.replace(CODE, "").split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pounds_and_pence_are_said_properly() {
        assert_eq!(words("It costs £3.50 today.", true), "It costs three pounds fifty today.");
        assert_eq!(words("£1 each", true), "one pound each");
        assert_eq!(words("£0.20", true), "twenty pence");
        assert_eq!(words("£1,250", true), "one thousand, two hundred and fifty pounds");
        assert_eq!(words("a £3m deal", true), "a three million pounds deal");
        assert_eq!(words("50p", true), "fifty pence");
        assert_eq!(words("costs in £", true), "costs in pounds");
    }

    #[test]
    fn numbers_are_said_the_british_way() {
        assert_eq!(number(105, true), "one hundred and five");
        assert_eq!(number(1005, true), "one thousand and five");
        assert_eq!(number(1_234_567, true), "one million, two hundred and thirty-four thousand, five hundred and sixty-seven");
        assert_eq!(words("3.14 and 12%", true), "three point one four and twelve per cent");
        assert_eq!(words("Call 07700 900123", true), "Call zero seven seven zero zero nine zero zero one two three");
        assert_eq!(words("the 21st time", true), "the twenty-first time");
    }

    #[test]
    fn dates_and_years_are_said_as_people_say_them() {
        assert_eq!(words("on 1 October 2026.", true), "on the first of October twenty twenty-six.");
        assert_eq!(words("in 1984", true), "in nineteen eighty-four");
        assert_eq!(words("in 2005", true), "in two thousand and five");
        assert_eq!(words("in 1905", true), "in nineteen oh five");
        assert_eq!(words("2026 people", true), "two thousand and twenty-six people");
    }

    #[test]
    fn american_english_is_said_the_american_way() {
        assert_eq!(number(1205, false), "one thousand two hundred five");
        assert_eq!(words("$3.50", false), "three dollars and fifty cents");
        assert_eq!(words("$0.20", false), "twenty cents");
        assert_eq!(words("12%", false), "twelve percent");
        assert_eq!(words("on October 1, 2026.", false), "on October first, twenty twenty-six.");
        assert_eq!(words("in 2005", false), "in two thousand five");
        assert_eq!(words("50p", false), "fifty p");
    }

    #[test]
    fn a_month_then_a_day_is_said_the_british_way_too() {
        assert_eq!(words("on October 1, 2026.", true), "on October the first, twenty twenty-six.");
    }
}
