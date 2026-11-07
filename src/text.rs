// Text tidying and chunking for the voice, ported from Supertonic's Rust example (MIT, Supertone Inc.).
// SPDX-License-Identifier: GPL-3.0-or-later
use unicode_normalization::UnicodeNormalization;

pub const LANGS: &[&str] = &["en", "ko", "ja", "ar", "bg", "cs", "da", "de", "el", "es", "et", "fi", "fr", "hi", "hr", "hu", "id", "it", "lt", "lv", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "tr", "uk", "vi", "na"];

const MAX_CHUNK: usize = 300;
const ABBREVIATIONS: &[&str] = &["Dr.", "Mr.", "Mrs.", "Ms.", "Prof.", "Sr.", "Jr.", "St.", "Ave.", "Rd.", "Blvd.", "Dept.", "Inc.", "Ltd.", "Co.", "Corp.", "etc.", "vs.", "i.e.", "e.g.", "Ph.D."];
const STOPS: [char; 6] = ['.', '!', '?', '。', '！', '？'];
const COMMAS: [char; 3] = [',', '、', '，'];

pub fn is_lang(lang: &str) -> bool {
    LANGS.contains(&lang)
}

fn emoji(c: char) -> bool {
    matches!(c as u32, 0x1F600..=0x1F64F | 0x1F300..=0x1F5FF | 0x1F680..=0x1F6FF | 0x1F700..=0x1F77F | 0x1F780..=0x1F7FF | 0x1F800..=0x1F8FF | 0x1F900..=0x1F9FF | 0x1FA00..=0x1FA6F | 0x1FA70..=0x1FAFF | 0x2600..=0x26FF | 0x2700..=0x27BF | 0x1F1E6..=0x1F1FF)
}

pub fn prepare(text: &str, lang: &str) -> Result<String, String> {
    if !is_lang(lang) {
        return Err(format!("unknown language: {lang}"));
    }
    let mut t: String = text.nfkd().filter(|c| !emoji(*c) && *c != '\u{301}').collect();
    for (from, to) in [("–", "-"), ("\u{2010}", "-"), ("—", "-"), ("_", " "), ("\u{201C}", "\""), ("\u{201D}", "\""), ("\u{2018}", "'"), ("\u{2019}", "'"), ("`", "'"), ("[", " "), ("]", " "), ("|", " "), ("/", " "), ("#", " "), ("→", " "), ("←", " "), ("©", ""), ("\\", ""), ("@", " at "), ("e.g.,", "for example, "), ("i.e.,", "that is, ")] {
        t = t.replace(from, to);
    }
    for p in [",", ".", "!", "?", ";", ":"] {
        t = t.replace(&format!(" {p}"), p);
    }
    for q in ["\"\"", "''"] {
        while t.contains(q) {
            t = t.replace(q, &q[..1]);
        }
    }
    let mut t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() {
        return Err("nothing to say".into());
    }
    if !t.ends_with(['.', '!', '?', ';', ':', ',', '\'', '"', ')', ']', '}', '…', '。', '」', '』', '】', '〉', '》', '›', '»', '！', '？']) {
        t.push('.');
    }
    Ok(format!("<{lang}>{t}</{lang}>"))
}

pub fn chunk(text: &str, max_len: usize) -> Vec<String> {
    let max_len = max_len.max(1);
    let mut chunks = Vec::new();
    for para in paragraphs(text.trim()) {
        if len(&para) <= max_len {
            chunks.push(para);
            continue;
        }
        let mut current = String::new();
        for sentence in sentences(&para) {
            let sentence = sentence.trim();
            if sentence.is_empty() {
                continue;
            }
            if len(sentence) > max_len {
                flush(&mut current, &mut chunks);
                for part in sentence.split(COMMAS).map(str::trim).filter(|p| !p.is_empty()) {
                    if len(part) > max_len {
                        flush(&mut current, &mut chunks);
                        by_words(part, max_len, &mut chunks);
                    } else {
                        join(&mut current, part, ", ", max_len, &mut chunks);
                    }
                }
            } else {
                join(&mut current, sentence, " ", max_len, &mut chunks);
            }
        }
        flush(&mut current, &mut chunks);
    }
    chunks
}

pub fn lead_pieces(words: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut now = String::new();
    let mut chars = words.chars().peekable();
    while let Some(c) = chars.next() {
        now.push(c);
        let n = now.trim().chars().count();
        let stops_here = c != '.' || chars.peek().is_none_or(|n| n.is_whitespace());
        let title = c == '.' && matches!(now.trim_end_matches('.').rsplit(' ').next(), Some("Dr" | "Mr" | "Mrs" | "Ms" | "St" | "Prof"));
        let sentence_end = matches!(c, '.' | '!' | '?' | '\n') && stops_here && !title;
        let first = out.is_empty();
        let end = sentence_end && n > 1
            || (c == ',' || c == ';' || c == ':') && ((first && n >= 12) || n > 120)
            || (first && n >= 40 && c == ' ')
            || n > 250;
        if end && !now.trim().is_empty() {
            out.push(std::mem::take(&mut now).trim().to_string());
        }
    }
    if !now.trim().is_empty() {
        out.push(now.trim().to_string());
    }
    out
}

pub fn default_max(lang: &str) -> usize {
    if lang == "ko" || lang == "ja" { 120 } else { MAX_CHUNK }
}

fn len(s: &str) -> usize {
    s.chars().count()
}

fn flush(current: &mut String, chunks: &mut Vec<String>) {
    if !current.is_empty() {
        chunks.push(current.trim().to_string());
        current.clear();
    }
}

fn join(current: &mut String, piece: &str, sep: &str, max_len: usize, chunks: &mut Vec<String>) {
    if len(current) + len(sep) + len(piece) > max_len {
        flush(current, chunks);
    }
    if !current.is_empty() {
        current.push_str(sep);
    }
    current.push_str(piece);
}

fn by_words(part: &str, max_len: usize, chunks: &mut Vec<String>) {
    let mut line = String::new();
    for word in part.split_whitespace() {
        let mut rest: Vec<char> = word.chars().collect();
        while rest.len() > max_len {
            flush(&mut line, chunks);
            chunks.push(rest.drain(..max_len).collect());
        }
        let word: String = rest.into_iter().collect();
        if !word.is_empty() {
            join(&mut line, &word, " ", max_len, chunks);
        }
    }
    flush(&mut line, chunks);
}

fn paragraphs(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for line in text.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() {
            if !cur.is_empty() {
                out.push(cur.join("\n").trim().to_string());
                cur.clear();
            }
        } else {
            cur.push(line);
        }
    }
    out
}

fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut last = 0;
    let mut iter = text.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        if !STOPS.contains(&c) {
            continue;
        }
        let mut end = i + c.len_utf8();
        while let Some(&(j, w)) = iter.peek() {
            if !w.is_whitespace() {
                break;
            }
            end = j + w.len_utf8();
            iter.next();
        }
        let wide = !c.is_ascii();
        if wide || (end > i + 1 && !ABBREVIATIONS.iter().any(|a| format!("{}{c}", text[last..i].trim()).ends_with(a))) {
            out.push(text[last..end].to_string());
            last = end;
        }
    }
    if last < text.len() {
        out.push(text[last..].to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tidies_and_tags() {
        assert_eq!(prepare("Save , button", "en").unwrap(), "<en>Save, button.</en>");
        assert_eq!(prepare("Hello\u{2019}s  world! \u{1F600}", "en").unwrap(), "<en>Hello's world!</en>");
        assert_eq!(prepare("mail me @ home", "en").unwrap(), "<en>mail me at home.</en>");
        assert_eq!(prepare("he said 'hi'", "en").unwrap(), "<en>he said 'hi'</en>");
    }

    #[test]
    fn nothing_to_say_and_unknown_language_are_refused() {
        assert_eq!(prepare("  \n ", "en").unwrap_err(), "nothing to say");
        assert_eq!(prepare("\u{1F600}", "en").unwrap_err(), "nothing to say");
        assert!(prepare("hi", "xx").is_err());
    }

    #[test]
    fn short_text_is_one_chunk_and_empty_is_none() {
        assert_eq!(chunk("Hello there.", 300), vec!["Hello there."]);
        assert!(chunk("  ", 300).is_empty());
    }

    #[test]
    fn a_point_inside_a_number_or_title_does_not_end_a_piece() {
        assert_eq!(lead_pieces("It is 3.5 million. Dr. Smith agrees."), vec!["It is 3.5 million.", "Dr. Smith agrees."]);
    }

    #[test]
    fn lead_pieces_are_cut_at_sentences() {
        assert_eq!(lead_pieces("One. Two! Three"), vec!["One.", "Two!", "Three"]);
        assert_eq!(lead_pieces("Hello there, my friend. Bye now."), vec!["Hello there,", "my friend.", "Bye now."]);
        assert_eq!(lead_pieces("Hi, you. Yes, no, maybe so, but only if I may."), vec!["Hi, you.", "Yes, no, maybe so, but only if I may."]);
    }

    #[test]
    fn paragraphs_split() {
        assert_eq!(chunk("One.\n\nTwo.", 300), vec!["One.", "Two."]);
    }

    #[test]
    fn long_text_splits_on_sentences_not_abbreviations() {
        let text = "Dr. Smith went home. He slept. Then he woke up and ate.";
        let c = chunk(text, 25);
        assert!(c.iter().all(|p| len(p) <= 25), "{c:?}");
        assert!(c[0].starts_with("Dr. Smith"), "{c:?}");
        assert_eq!(c.join(" "), text);
    }

    #[test]
    fn long_part_after_a_comma_keeps_the_order() {
        let c = chunk("First bit, then a very long second part with many words in it, last.", 20);
        assert!(c.iter().all(|p| len(p) <= 20), "{c:?}");
        assert_eq!(c[0], "First bit");
        assert_eq!(c.last().unwrap(), "last.");
        let spoken = c.join(" ");
        assert!(spoken.find("First").unwrap() < spoken.find("second").unwrap());
        assert!(spoken.find("second").unwrap() < spoken.find("last").unwrap());
    }

    #[test]
    fn very_long_word_is_cut() {
        let c = chunk(&"a".repeat(50), 12);
        assert!(c.iter().all(|p| len(p) <= 12), "{c:?}");
        assert_eq!(c.concat(), "a".repeat(50));
    }

    #[test]
    fn japanese_splits_on_its_own_full_stop_and_counts_characters() {
        let text = "今日は天気がいいです。".repeat(30);
        let c = chunk(&text, 40);
        assert!(c.len() > 1);
        assert!(c.iter().all(|p| len(p) <= 40), "{c:?}");
        assert_eq!(c.concat().replace(' ', ""), text);
    }

    #[test]
    fn separator_is_counted() {
        let c = chunk("aaaa, bbbb, cccc, dddd, eeee eeee eeee eeee eeee", 10);
        assert!(c.iter().all(|p| len(p) <= 10), "{c:?}");
    }
}
