//! Bounded, offline lexical retrieval. No embeddings, network requests or writes.
use super::Memory;
use regex::Regex;
use std::{collections::BTreeSet, sync::LazyLock};

const CLIP_ALIASES: &[&str] = &[
    "剪辑",
    "剪片",
    "剪个片",
    "剪视频",
    "剪个视频",
    "把视频剪",
    "剪一段视频",
    "video editing",
];
const IMAGE_ALIASES: &[&str] = &[
    "生图",
    "出图",
    "图片生成",
    "生成图片",
    "生成一个图片",
    "生成一张图",
    "做一张图",
    "做张图",
    "画一张图",
    "画张图",
    "画个封面",
    "做张封面图",
    "文章配图",
    "封面制作",
    "image generation",
];
static IMAGE_REQUEST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:画|做|生成|出|配)(?:[一两二三1-9]?[张幅个])[\p{Han}A-Za-z0-9 -]{0,24}(?:图像|图片|头图|封面图|封面|插画|海报|头像|图)(?:[，。！、？,.!?；;\s]|$)")
        .expect("static image request pattern")
});
static WORKFLOW_FILLER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?:按照|按|照|沿用)(?:我们|咱们)?(?:之前|以前|上次|上回|原来|平时)",
        r"(?:的?(?:方式|做法|办法|流程|习惯)|那套)|[按照]老办法|照旧|完成一下",
    ))
    .expect("static workflow filler pattern")
});

fn clip_topic(text: &str) -> bool {
    CLIP_ALIASES.iter().any(|term| text.contains(term))
}

fn image_topic(text: &str) -> bool {
    IMAGE_ALIASES.iter().any(|term| text.contains(term)) || IMAGE_REQUEST.is_match(text)
}

fn image_memory_title(title: &str) -> bool {
    image_topic(title)
        || ["头图", "封面图", "插画"]
            .iter()
            .any(|term| title.contains(term))
}

fn cjk(c: char) -> bool {
    ('\u{3400}'..='\u{9fff}').contains(&c)
}
fn tokens(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut word = String::new();
    let mut previous = None;
    for c in text.to_lowercase().chars().take(4000) {
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
            previous = None;
        } else {
            if word.len() > 1 {
                out.insert(std::mem::take(&mut word));
            }
            word.clear();
            if cjk(c) {
                if let Some(p) = previous {
                    out.insert(format!("{p}{c}"));
                }
                previous = Some(c);
            } else {
                previous = None;
            }
        }
    }
    if word.len() > 1 {
        out.insert(word);
    }
    out.retain(|s| {
        ![
            "the", "and", "for", "please", "帮我", "一下", "请问", "这个", "那个", "我们", "可以",
            "任务", "怎么", "请你",
        ]
        .contains(&s.as_str())
    });
    out
}

pub fn ranked<'a>(
    memories: impl Iterator<Item = &'a Memory>,
    query: &str,
) -> Vec<(&'a Memory, u32)> {
    let query = query.trim().to_lowercase();
    // The reference asks to reuse a method; the remaining words identify the task.
    let subject = WORKFLOW_FILLER.replace_all(&query, " ");
    let subject = subject.trim();
    let clip = clip_topic as fn(&str) -> bool;
    let topics = [(clip, clip), (image_topic, image_memory_title)]
        .into_iter()
        .filter(|(matches, _)| matches(subject))
        .collect::<Vec<_>>();
    let terms = tokens(subject);
    let mut found: Vec<_> = memories
        .filter_map(|m| {
            if query.is_empty() {
                return Some((m, 1));
            }
            let title = m.title.to_lowercase();
            let body = m.content.to_lowercase();
            let title_terms = tokens(&title);
            let body_terms = tokens(&body);
            let hits = terms
                .iter()
                .filter(|t| title_terms.contains(*t) || body_terms.contains(*t))
                .count();
            let title_hits = terms.intersection(&title_terms).count();
            let exact = !subject.is_empty() && (title.contains(subject) || body.contains(subject));
            let topic_score = topics
                .iter()
                .map(|(matches_body, matches_title)| {
                    if matches_title(&title) {
                        20
                    } else if matches_body(&body) {
                        8
                    } else {
                        0
                    }
                })
                .sum::<usize>();
            let score =
                (hits * 10 + title_hits * 15 + usize::from(exact) * 100 + topic_score) as u32;
            // One common Chinese bigram in a long sentence isn't sufficient recall.
            (exact
                || topic_score > 0
                || hits >= 2
                || hits == 1 && (terms.len() <= 2 || title_hits > 0))
                .then_some((m, score))
        })
        .collect();
    found.sort_by(|(a, sa), (b, sb)| {
        sb.cmp(sa)
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    found.truncate(12);
    found
}
