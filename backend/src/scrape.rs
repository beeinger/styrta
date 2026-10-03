//! Parser for the ROPS Biblioteka innowacji społecznych pages.
//!
//! The markup is not well-formed (unclosed paragraphs, layout tables), so this
//! walks the raw HTML instead of a DOM.

pub const ORIGIN: &str = "https://rops.krakow.pl";

pub const CATEGORIES: &[(&str, &str)] = &[
    ("dla-seniorow", "Dla seniorów"),
    (
        "dla-dzieci-mlodziezy-i-rodziny",
        "Dla dzieci, młodzieży i rodziny",
    ),
    (
        "dla-osob-o-ograniczonej-mobilnosci",
        "Dla osób o ograniczonej mobilności",
    ),
    (
        "dla-osob-z-niepelnosprawnoscia-sensoryczna",
        "Dla osób z niepełnosprawnością sensoryczną",
    ),
    ("dla-zdrowia-i-medycyny", "Dla zdrowia i medycyny"),
    ("dla-rynku-pracy", "Dla rynku pracy"),
    ("dla-cudzoziemcow", "Dla cudzoziemców"),
    (
        "dla-osob-w-kryzysie-bezdomnosci",
        "Dla osób w kryzysie bezdomności",
    ),
    (
        "dla-osob-z-niepelnosprawnoscia-intelektualna",
        "Dla osób z niepełnosprawnością intelektualną",
    ),
];

pub fn category_url(slug: &str) -> String {
    format!("{ORIGIN}/innowacje-spoleczne/biblioteka-innowacji-spolecznych/{slug}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub kind: &'static str,
    pub url: String,
    pub label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Card {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub page_url: String,
    pub links: Vec<Link>,
}

#[derive(Clone, Debug)]
pub struct Section {
    pub heading: String,
    pub body: String,
}

#[derive(Clone, Debug)]
pub struct Detail {
    pub title: String,
    pub intro: String,
    pub sections: Vec<Section>,
    pub links: Vec<Link>,
    pub licence: Option<String>,
    pub zip_url: Option<String>,
}

pub fn parse_category_page(category_slug: &str, html: &str) -> Vec<Card> {
    let mut cards = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for chunk in html.split(r#"class="news-list__item""#).skip(1) {
        // The last card runs on into the site footer. Cut before it.
        let chunk = chunk
            .split("m-publications")
            .next()
            .unwrap_or(chunk)
            .split("<footer")
            .next()
            .unwrap_or(chunk);
        let Some(href) = first_innovation_href(category_slug, chunk) else {
            continue;
        };
        let Some(slug) = innovation_key(&href) else {
            continue;
        };
        if !seen.insert(slug.clone()) {
            continue;
        }
        let title = text_after(chunk, r#"news-list__title">"#).unwrap_or_else(|| slug.clone());
        let summary = card_summary(chunk);
        let mut links = links_from_html(chunk);
        links.retain(|l| l.kind != "page");
        links.insert(
            0,
            Link {
                kind: "page",
                url: href.clone(),
                label: Some("strona innowacji".into()),
            },
        );
        dedupe_links(&mut links);
        cards.push(Card {
            slug,
            title,
            summary,
            page_url: href,
            links,
        });
    }
    cards
}

pub fn parse_detail(page_url: &str, html: &str) -> Detail {
    let title = match_title(html).unwrap_or_default();
    let slice = article_slice(html);
    let (intro_html, rest) = slice
        .split_once("<h4")
        .map(|(a, b)| (a, format!("<h4{b}")))
        .unwrap_or((slice.as_str(), String::new()));
    let intro = clean_intro(&html_to_text(intro_html));
    let sections = parse_sections(&rest);
    let mut links = links_from_html(&slice);
    if !links.iter().any(|l| l.kind == "page") {
        links.insert(
            0,
            Link {
                kind: "page",
                url: absolute(page_url).unwrap_or_else(|| page_url.to_string()),
                label: Some("strona innowacji".into()),
            },
        );
    }
    dedupe_links(&mut links);
    let licence = licence_from_links(&links);
    let zip_url = links
        .iter()
        .find(|l| l.kind == "zip")
        .map(|l| l.url.clone());
    Detail {
        title,
        intro,
        sections,
        links,
        licence,
        zip_url,
    }
}

pub fn title_key(title: &str) -> String {
    title
        .chars()
        .flat_map(|c| c.to_lowercase())
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn absolute(href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty()
        || href.starts_with('#')
        || href.starts_with("javascript:")
        || href.starts_with("mailto:")
    {
        return None;
    }
    if href.starts_with("https://") || href.starts_with("http://") {
        return Some(href.to_string());
    }
    if let Some(rest) = href.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if let Some(path) = href.strip_prefix('/') {
        return Some(format!("{ORIGIN}/{path}"));
    }
    Some(format!("{ORIGIN}/{href}"))
}

pub fn classify_href(url: &str) -> &'static str {
    let u = url.to_ascii_lowercase();
    if u.contains("youtube.com") || u.contains("youtu.be") || u.contains("vimeo.com") {
        "video"
    } else if u.contains("creativecommons.org")
        || u.contains("zasady_wykorzystania")
        || u.contains("zasady-wykorzystania")
    {
        "licence"
    } else if path_has_ext(&u, ".zip") {
        "zip"
    } else if path_has_ext(&u, ".pdf") {
        "pdf"
    } else {
        "other"
    }
}

pub fn licence_name(url: &str) -> String {
    let u = url.to_ascii_lowercase();
    if u.contains("creativecommons.org/licenses/by-nc-sa") {
        "CC BY-NC-SA 4.0".into()
    } else if u.contains("creativecommons.org/licenses/by-nc") {
        "CC BY-NC 4.0".into()
    } else if u.contains("creativecommons.org/licenses/by-sa") {
        "CC BY-SA 4.0".into()
    } else if u.contains("creativecommons.org/licenses/by") {
        "CC BY 4.0".into()
    } else if u.contains("miis") || u.contains("zasady") {
        "Zasady wykorzystania innowacji MIIS".into()
    } else {
        url.to_string()
    }
}

fn licence_from_links(links: &[Link]) -> Option<String> {
    links
        .iter()
        .find(|l| l.kind == "licence")
        .map(|l| licence_name(&l.url))
}

fn path_has_ext(url: &str, ext: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.ends_with(ext)
}

fn first_innovation_href(category_slug: &str, chunk: &str) -> Option<String> {
    let needle = format!("/{category_slug},");
    for href in hrefs(chunk) {
        if href.contains(&needle) || href.contains(&format!("{category_slug},")) {
            return absolute(&href);
        }
    }
    None
}

fn innovation_key(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let seg = path.trim_end_matches('/').rsplit('/').next()?;
    if seg.contains(',') {
        Some(seg.to_string())
    } else {
        None
    }
}

fn card_summary(chunk: &str) -> String {
    let Some(idx) = chunk.find(r#"news-list__desc">"#) else {
        return String::new();
    };
    let rest = &chunk[idx + r#"news-list__desc">"#.len()..];
    let end = rest.find("<table").unwrap_or(rest.len().min(1500));
    let text = html_to_text(&rest[..end]);
    let mut out = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_lowercase();
        if lower.starts_with("innowacja wybrana")
            || lower.starts_with("dowiedz")
            || lower.starts_with("zobacz")
            || lower.starts_with("pobierz")
        {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
        if out.chars().count() > 280 {
            break;
        }
    }
    out
}

fn links_from_html(html: &str) -> Vec<Link> {
    let mut links = Vec::new();
    for href in hrefs(html) {
        let Some(url) = absolute(&href) else { continue };
        if is_category_index(&url) {
            continue;
        }
        let kind = classify_href(&url);
        if kind == "other" && !url.contains("rops.krakow.pl") && !url.contains("youtube") {
            // Keep external project sites; drop pure in-site chrome later if needed.
        }
        let label = link_label(kind, &url);
        links.push(Link {
            kind,
            url,
            label: Some(label),
        });
    }
    for src in img_srcs(html) {
        if src.contains("iKONY_na_www") || src.contains("iKONY") {
            continue;
        }
        let Some(url) = absolute(&src) else { continue };
        links.push(Link {
            kind: "qr",
            url,
            label: Some("otwórz w telefonie (kod QR)".into()),
        });
    }
    links
}

fn link_label(kind: &str, url: &str) -> String {
    match kind {
        "video" => "film".into(),
        "zip" => "pobierz materiały".into(),
        "pdf" => file_name(url).unwrap_or_else(|| "pdf".into()),
        "licence" => "zasady wykorzystania".into(),
        "page" => "strona innowacji".into(),
        _ => file_name(url).unwrap_or_else(|| "odnośnik".into()),
    }
}

fn is_category_index(url: &str) -> bool {
    let url = url.trim_end_matches('/');
    CATEGORIES
        .iter()
        .any(|(slug, _)| url.ends_with(&format!("/biblioteka-innowacji-spolecznych/{slug}")))
}

fn file_name(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let name = path.rsplit('/').next()?;
    if name.is_empty() {
        None
    } else {
        Some(decode_basic(name))
    }
}

fn dedupe_links(links: &mut Vec<Link>) {
    let rank = |kind: &str| match kind {
        "zip" => 0,
        "video" => 1,
        "pdf" => 2,
        "licence" => 3,
        "qr" => 4,
        "page" => 5,
        _ => 6,
    };
    links.sort_by(|a, b| rank(a.kind).cmp(&rank(b.kind)).then(a.url.cmp(&b.url)));
    let mut seen = std::collections::BTreeSet::new();
    links.retain(|l| seen.insert(l.url.clone()));
}

fn hrefs(html: &str) -> Vec<String> {
    attr_values(html, "href")
}

fn img_srcs(html: &str) -> Vec<String> {
    attr_values(html, "src")
}

fn attr_values(html: &str, attr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = html.to_ascii_lowercase();
    let needle = format!("{attr}=\"");
    let mut start = 0;
    while let Some(rel) = lower[start..].find(&needle) {
        let value_at = start + rel + needle.len();
        let Some(end) = html[value_at..].find('"') else {
            break;
        };
        out.push(html[value_at..value_at + end].to_string());
        start = value_at + end + 1;
    }
    out
}

fn text_after(html: &str, marker: &str) -> Option<String> {
    let idx = html.find(marker)?;
    let rest = &html[idx + marker.len()..];
    let end = rest.find('<').unwrap_or(rest.len().min(200));
    let text = html_to_text(&rest[..end]);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn match_title(html: &str) -> Option<String> {
    const MARKER: &str = r#"<h2 class="page-title">"#;
    let idx = html.find(MARKER)?;
    let rest = &html[idx + MARKER.len()..];
    let end = rest.find("</h2>")?;
    let text = html_to_text(&rest[..end]);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn article_slice(html: &str) -> String {
    const START: &str = r#"<div class="text-content">"#;
    let Some(idx) = html.find(START) else {
        return String::new();
    };
    let rest = &html[idx + START.len()..];
    if let Some(end) = rest.find("<h2") {
        rest[..end].to_string()
    } else {
        rest.to_string()
    }
}

fn parse_sections(html: &str) -> Vec<Section> {
    let mut sections = Vec::new();
    for part in html.split("<h4").skip(1) {
        let Some(close) = part.find("</h4>") else {
            continue;
        };
        let Some(open_end) = part[..close].rfind('>') else {
            continue;
        };
        let heading = html_to_text(&part[open_end + 1..close]);
        let body = html_to_text(&part[close + 5..]);
        if heading.is_empty() || body.is_empty() {
            continue;
        }
        sections.push(Section { heading, body });
    }
    sections
}

fn clean_intro(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_lowercase();
        if matches!(
            lower.as_str(),
            "dowiedz się więcej"
                | "zobacz film"
                | "pobierz materiały"
                | "sprawdź zasady wykorzystania"
                | "otwórz w telefonie"
        ) || lower.starts_with("dowiedz")
            || lower.starts_with("zobacz film")
            || lower.starts_with("pobierz materia")
            || lower.starts_with("sprawdź zasady")
            || lower.starts_with("otwórz")
        {
            continue;
        }
        lines.push(line.to_string());
    }
    lines.join("\n")
}

pub fn html_to_text(html: &str) -> String {
    let broken = html
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("</p>", "\n")
        .replace("</div>", "\n")
        .replace("</li>", "\n")
        .replace("</h4>", "\n")
        .replace("</tr>", "\n")
        .replace("<li>", "\n- ");
    let stripped = strip_tags(&broken);
    let decoded = decode_entities(&stripped);
    let mut lines = Vec::new();
    for line in decoded.lines() {
        let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            if lines.last().is_some_and(|l: &String| !l.is_empty()) {
                lines.push(String::new());
            }
        } else {
            lines.push(collapsed);
        }
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    drop_chrome(&lines.join("\n"))
}

fn drop_chrome(text: &str) -> String {
    text.lines()
        .filter(|line| !is_chrome_line(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_chrome_line(line: &str) -> bool {
    let lower = line
        .trim()
        .trim_matches(|c: char| !c.is_alphanumeric() && c != ' ')
        .to_lowercase();
    matches!(
        lower.as_str(),
        "się więcej"
            | "zobacz"
            | "film"
            | "pobierz"
            | "materiały"
            | "wykorzystania"
            | "w telefonie"
            | "powrót"
            | "drukuj"
            | "dowiedz się więcej"
            | "zobacz film"
            | "pobierz materiały"
            | "otwórz w telefonie"
            | "sprawdź zasady"
            | "sprawdź zasady wykorzystania"
    )
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        if end > 12 {
            out.push('&');
            rest = &rest[1..];
            continue;
        }
        let entity = &rest[1..end];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ if entity.starts_with('#') => decode_numeric(entity),
            _ => None,
        };
        if let Some(ch) = ch {
            out.push(ch);
            rest = &rest[end + 1..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

fn decode_numeric(entity: &str) -> Option<char> {
    let body = entity.strip_prefix('#')?;
    let code = if let Some(hex) = body.strip_prefix(['x', 'X']) {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        body.parse().ok()?
    };
    char::from_u32(code)
}

fn decode_basic(text: &str) -> String {
    decode_entities(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARD: &str = r#"
<div class="news-list__item">
  <a href="/innowacje-spoleczne/biblioteka-innowacji-spolecznych/dla-seniorow,bawita" class="news-list__title">BaWita</a>
  <p class="news-list__desc"><p>BaWita - tablica manipulacyjno terapeutyczna</p>
  <table>
    <a href="/mpliki/IS/BIBLIOTEKA_INNOWACJI_SPOECZNYCH/ROPS_Folder_IN_BaWita_v15_www.pdf"><img src="/mpliki/IS/iKONY_na_www/lupa.png"></a>
    <a href="https://www.youtube.com/watch?v=o7UhDlebLJo"><img src="/mpliki/IS/iKONY_na_www/play_black.png"></a>
    <a href="https://rops.krakow.pl/pliki/IS/bibloteka/bawita.zip"></a>
    <a href="https://creativecommons.org/licenses/by/4.0/deed.pl"><img src="/mpliki/IS/iKONY_na_www/CC_BY.png"></a>
    <img src="/mpliki/IS/BIBLIOTEKA_INNOWACJI_SPOECZNYCH/bawita.png">
  </table>
</div>
<div class="m-publications__list">
  <a href="/dzial-publikacje/esoes-2024-1">es.O.es</a>
  <img src="/mpliki/footer/not-a-qr.png">
</div>
"#;

    const DETAIL: &str = r#"
<h2 class="page-title">BaWita</h2>
<div class="text-content">
<p><strong>INNOWACJA WYBRANA DO UPOWSZECHNIANIA</strong></p>
<table>
<a href="https://rops.krakow.pl/pliki/IS/bibloteka/bawita.zip"></a>
<a href="https://www.youtube.com/watch?v=o7UhDlebLJo"></a>
<a href="https://creativecommons.org/licenses/by/4.0/deed.pl"></a>
<img src="/mpliki/IS/BIBLIOTEKA_INNOWACJI_SPOECZNYCH/bawita.png">
</table>
<h4>1. Na czym polega rozwiązanie?</h4>
<p>Tablica manipulacyjna dla osób z demencją.</p>
<h4>3. Grupa docelowa</h4>
<p>Seniorzy.</p>
<h4>6. Autorzy</h4>
<p>Maria Lorenc</p>
<p>Powrót</p>
<p>Drukuj</p>
</div>
<h2>Publikacje</h2>
"#;

    #[test]
    fn category_card_keeps_links() {
        let cards = parse_category_page("dla-seniorow", CARD);
        assert_eq!(cards.len(), 1);
        let card = &cards[0];
        assert_eq!(card.slug, "dla-seniorow,bawita");
        assert_eq!(card.title, "BaWita");
        assert!(card.summary.contains("tablica"));
        let kinds: Vec<_> = card.links.iter().map(|l| l.kind).collect();
        assert!(kinds.contains(&"zip"));
        assert!(kinds.contains(&"video"));
        assert!(kinds.contains(&"pdf"));
        assert!(kinds.contains(&"licence"));
        assert!(kinds.contains(&"qr"));
        assert!(kinds.contains(&"page"));
        assert!(!card.links.iter().any(|l| l.url.contains("esoes")));
        assert!(!card.links.iter().any(|l| l.url.contains("not-a-qr")));
        assert_eq!(
            card.links.iter().find(|l| l.kind == "zip").unwrap().url,
            "https://rops.krakow.pl/pliki/IS/bibloteka/bawita.zip"
        );
    }

    #[test]
    fn detail_sections_and_licence() {
        let detail = parse_detail("https://rops.krakow.pl/x", DETAIL);
        assert_eq!(detail.title, "BaWita");
        assert!(detail.intro.contains("INNOWACJA WYBRANA"));
        assert_eq!(detail.sections.len(), 3);
        assert!(detail.sections[0].body.contains("demencją"));
        assert!(!detail.sections.iter().any(|s| s.body.contains("Drukuj")));
        assert_eq!(detail.licence.as_deref(), Some("CC BY 4.0"));
        assert!(detail.zip_url.unwrap().ends_with("bawita.zip"));
        assert!(detail.links.iter().any(|l| l.kind == "qr"));
    }

    #[test]
    fn miis_licence_name() {
        let url = "https://rops.krakow.pl/mpliki/IS/BIBLIOTEKA_INNOWACJI_SPOECZNYCH/Zasady_wykorzystania_innowacji_MIIS.pdf";
        assert_eq!(classify_href(url), "licence");
        assert_eq!(licence_name(url), "Zasady wykorzystania innowacji MIIS");
    }
}
