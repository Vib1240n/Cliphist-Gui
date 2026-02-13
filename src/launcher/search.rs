use crate::desktop::{DesktopEntry, FREQUENCY};

pub fn fuzzy_match(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }

    let q = query.to_lowercase();
    let t = text.to_lowercase();

    if t == q {
        return Some(1000);
    }
    if t.starts_with(&q) {
        return Some(500 + (100 - q.len() as i32));
    }
    if t.contains(&q) {
        return Some(200);
    }

    let mut qi = q.chars().peekable();
    let mut score = 0;
    let mut consecutive = 0;

    for c in t.chars() {
        if qi.peek() == Some(&c) {
            qi.next();
            consecutive += 1;
            score += consecutive * 10;
        } else {
            consecutive = 0;
        }
    }

    if qi.peek().is_none() {
        Some(score)
    } else {
        None
    }
}

pub fn filter_entries(entries: &[DesktopEntry], query: &str) -> Vec<DesktopEntry> {
    if query.is_empty() {
        return entries.to_vec();
    }

    let mut matched: Vec<(DesktopEntry, i32)> = entries
        .iter()
        .filter_map(|e| {
            let name_score = fuzzy_match(query, &e.name);
            let desc_score = fuzzy_match(query, &e.description).map(|s| s / 2);
            let best = name_score.max(desc_score);
            best.map(|s| (e.clone(), s))
        })
        .collect();

    FREQUENCY.with(|f| {
        let freq = f.borrow();
        for (entry, score) in &mut matched {
            if let Some(&count) = freq.get(&entry.name) {
                *score += (count * 50) as i32;
            }
        }
    });

    matched.sort_by(|a, b| b.1.cmp(&a.1));
    matched.into_iter().map(|(e, _)| e).collect()
}

pub fn get_filtered_entry(
    entries: &[DesktopEntry],
    query: &str,
    idx: usize,
) -> Option<DesktopEntry> {
    let filtered = filter_entries(entries, query);
    filtered.get(idx).cloned()
}
