use super::*;

pub(crate) const COMMAND: &str = "repo_search_public";

fn valid_part(part: &str) -> bool {
    (1..=100).contains(&part.len())
        && part.as_bytes()[0].is_ascii_alphanumeric()
        && !part.contains("..")
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_query(query: &str) -> bool {
    match query.split_once('/') {
        Some((owner, name)) => valid_part(owner) && valid_part(name),
        None => valid_part(query),
    }
}

fn name_after_marker(text: &str) -> Option<&str> {
    let text = text.trim_start_matches(|c: char| c.is_whitespace() || matches!(c, ':' | '：'));
    let piece = match text.chars().next()? {
        open @ ('「' | '『' | '“' | '‘' | '"' | '\'' | '`') => {
            let close = match open {
                '「' => '」',
                '『' => '』',
                '“' => '”',
                '‘' => '’',
                _ => open,
            };
            text[open.len_utf8()..].split_once(close)?.0.trim()
        }
        _ => {
            let end = text
                .find(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '.' | '_' | '-' | '/'))
                .unwrap_or(text.len());
            if text[end..].chars().next().is_some_and(|separator| {
                !(separator.is_whitespace()
                    || matches!(
                        separator,
                        ',' | ';'
                            | '!'
                            | ')'
                            | '，'
                            | '。'
                            | '；'
                            | '、'
                            | '！'
                            | '？'
                            | '）'
                            | '\u{3400}'..='\u{9fff}'
                    ))
            }) {
                return None;
            }
            text[..end].trim_end_matches('.')
        }
    };
    (piece.len() >= 2 && piece.bytes().any(|b| b.is_ascii_alphabetic()) && valid_query(piece))
        .then_some(piece)
}

pub(super) fn mentioned_names(text: &str, links: &[String]) -> Vec<String> {
    const MARKERS: &[&str] = &[
        "项目名称叫",
        "项目名称是",
        "项目名字叫",
        "项目名叫",
        "项目名是",
        "项目名为",
        "项目叫做",
        "项目叫",
        "仓库名叫",
        "仓库名是",
        "仓库名为",
        "仓库叫做",
        "仓库叫",
        "project is called ",
        "project called ",
        "project named ",
        "repository is called ",
        "repository called ",
        "repository named ",
    ];
    let lowercase = text.to_ascii_lowercase();
    let mut mentions = Vec::new();
    for &marker in MARKERS {
        for (at, _) in lowercase.match_indices(marker) {
            if marker.is_ascii()
                && text[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                continue;
            }
            if let Some(name) = name_after_marker(&text[at + marker.len()..]) {
                mentions.push((at, name));
            }
        }
    }
    mentions.sort_unstable_by_key(|(at, _)| *at);
    let mut names: Vec<String> = Vec::new();
    for (_, name) in mentions {
        if names
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(name))
        {
            continue;
        }
        if links.iter().any(|url| {
            let slug = url.strip_prefix("https://github.com/").unwrap_or(url);
            slug.eq_ignore_ascii_case(name)
                || !name.contains('/')
                    && slug
                        .rsplit('/')
                        .next()
                        .is_some_and(|repo| repo.eq_ignore_ascii_case(name))
        }) {
            continue;
        }
        names.push(name.to_owned());
        if names.len() == video::MAX_REPOSITORY_REFS {
            break;
        }
    }
    names
}

pub(super) fn saved_names(result: &Value) -> Option<Vec<String>> {
    let values = result["project_names"].as_array()?;
    if values.len() > video::MAX_REPOSITORY_REFS {
        return None;
    }
    let mut names: Vec<String> = Vec::new();
    for value in values {
        let name = value.as_str().filter(|name| valid_query(name))?;
        if names.iter().any(|prior| prior.eq_ignore_ascii_case(name)) {
            return None;
        }
        names.push(name.into());
    }
    Some(names)
}

pub(super) fn append_names(text: &mut String, names: Vec<String>) {
    if names.is_empty() {
        return;
    }
    text.push_str("\n\n文字稿提到这些项目名，归属待核对：");
    for (index, name) in names.iter().enumerate() {
        text.push_str(&format!("\n{}. {name}", index + 1));
    }
    text.push_str("\n可引用这条回复说「找第 1 个项目」查询公开 GitHub 候选。");
}

pub(crate) fn candidates(value: &Value) -> Option<&[Value]> {
    let fields = value.as_object()?;
    let candidates = value["candidates"].as_array()?;
    if fields.len() != 3
        || value["status"] != "succeeded"
        || !value["partial"].is_boolean()
        || candidates.len() > 5
    {
        return None;
    }
    let mut urls = std::collections::HashSet::new();
    for candidate in candidates {
        let url = candidate["repository_url"].as_str()?;
        if !candidate
            .as_object()
            .is_some_and(|fields| fields.len() == 3)
            || video::repository_refs(url).first().map(String::as_str) != Some(url)
            || !candidate["archived"].is_boolean()
            || !candidate["stars"]
                .as_u64()
                .is_some_and(|stars| stars <= (1 << 53) - 1)
            || !urls.insert(url.to_ascii_lowercase())
        {
            return None;
        }
    }
    Some(candidates)
}

pub(crate) fn search_error(value: &Value) -> Option<(&'static str, &'static str)> {
    if value["status"] != "failed" {
        return None;
    }
    match value["error"]["code"].as_str()? {
        "SOURCE_RATE_LIMITED" => Some((
            "SOURCE_RATE_LIMITED",
            "GitHub 暂时限制了公开搜索。请稍后再查。",
        )),
        "SOURCE_INVALID" => Some(("SOURCE_INVALID", "GitHub 搜索响应无法核对。")),
        "SOURCE_HTTP_ERROR" => Some((
            "SOURCE_HTTP_ERROR",
            "GitHub 公共搜索返回错误，暂时无法给出候选。",
        )),
        "SOURCE_UNAVAILABLE" => Some(("SOURCE_UNAVAILABLE", "暂时无法读取 GitHub 公共搜索结果。")),
        "SOURCE_LIMIT" => Some(("SOURCE_LIMIT", "GitHub 搜索结果超过读取上限。")),
        "REPOSITORY_SEARCH_INVALID" => Some((
            "REPOSITORY_SEARCH_INVALID",
            "请使用 GitHub 仓库名或 owner/repo。",
        )),
        _ => None,
    }
}

pub(super) fn summary(result: &Value) -> String {
    if let Some((_, message)) = search_error(result) {
        return message.into();
    }
    let Some(candidates) = candidates(result) else {
        return "原搜索结果暂时无法核对。".into();
    };
    let partial = result["partial"] == true;
    if candidates.is_empty() {
        return if partial {
            "这次只取得部分搜索结果，暂时没看到同名候选。可以提供 owner/repo 再查。"
        } else {
            "没有找到同名的公开仓库。可以核对拼写，或提供 owner/repo。"
        }
        .into();
    }
    let mut text = "找到这些同名候选，请核对仓库归属。".to_string();
    for (index, candidate) in candidates.iter().enumerate() {
        let url = candidate["repository_url"].as_str().unwrap_or_default();
        let archived = if candidate["archived"] == true {
            " · 已归档"
        } else {
            ""
        };
        text.push_str(&format!(
            "\n{}. {url} · {} 星{archived}",
            index + 1,
            candidate["stars"]
        ));
    }
    if partial {
        text.push_str("\nGitHub 返回的结果可能还有其他同名项目。");
    }
    text.push_str("\n引用这条回复说「分析第 1 个项目」，或发送 /分析项目 原搜索任务编号 1。");
    text
}

impl Bridge {
    pub(super) async fn search_video_repository(
        &self,
        message: &Value,
        account: &Stored,
        text: &str,
    ) -> Option<Result<Action, String>> {
        const USAGE: &str = "请引用已转写的视频回复「找第 1 个项目」或「找项目 marked」，也可发 /找项目 原视频收件编号 #1。";
        let text = text.trim().trim_end_matches(['。', '！', '!']);
        let (video, query) = if let Some(words) = text
            .strip_prefix("/找项目")
            .filter(|suffix| suffix.is_empty() || suffix.starts_with(char::is_whitespace))
        {
            let Some((receipt, query)) = words.trim().split_once(char::is_whitespace) else {
                return Some(Err(USAGE.into()));
            };
            (receipt.to_owned(), query.trim())
        } else {
            let ordinal = text
                .strip_prefix("找第")
                .and_then(|tail| tail.strip_suffix("个项目"));
            let query = if let Some(ordinal) = ordinal {
                ordinal.trim()
            } else {
                text.strip_prefix("找项目")
                    .filter(|suffix| suffix.is_empty() || suffix.starts_with(char::is_whitespace))?
                    .trim()
            };
            let video = match self.quoted_task(account, message, true).await? {
                Ok(Action::Query(receipt, _, false)) => receipt,
                _ => return Some(Err(USAGE.into())),
            };
            (video, query)
        };
        let selection = if let Some(index) = query.strip_prefix('#') {
            Some(index)
        } else if text.starts_with("找第") {
            Some(query)
        } else {
            None
        };
        if !video::receipt_id(&video) || (selection.is_none() && !valid_query(query)) {
            return Some(Err(USAGE.into()));
        }
        let source = account.logical_key();
        let original = self
            .state
            .lock()
            .await
            .stored
            .records
            .iter()
            .find(|record| {
                record.id == video
                    && record.is_video()
                    && record.status == "completed"
                    && record.args["source"] == source
            })
            .and_then(|record| record.result.clone());
        let Some(original) = original else {
            return Some(Err(
                "没有找到当前微信来源已转写完成的原视频。请先核对视频结果。".into(),
            ));
        };
        let query = if let Some(index) = selection {
            let Some(ordinal) = video::repository_ordinal(index) else {
                return Some(Err("请使用原视频项目名列表中的序号。".into()));
            };
            let names = saved_names(&original).or_else(|| {
                video::legacy_repository_hints(&video, &self.workspace, &original["artifact"])
                    .map(|hints| hints.names)
            });
            let Some(name) = names.and_then(|names| names.into_iter().nth(ordinal - 1)) else {
                return Some(Err(
                    "原视频没有这个可核对的项目名。请查看原结果，或明确写出要查找的名字。".into(),
                ));
            };
            name
        } else {
            query.into()
        };
        Some(Ok(Action::Call(COMMAND.into(), json!({"query":query}))))
    }

    pub(super) async fn search_candidate(
        &self,
        receipt: &str,
        job: &str,
        ordinal: usize,
        generation: u64,
    ) -> Result<String, String> {
        let observed = self
            .observe_query(
                generation,
                self.request("jobs.get", None, json!({}), receipt, Some(job)),
            )
            .await
            .ok_or("微信连接已变化，未开始分析。")?
            .map_err(|_| "原搜索结果暂时无法读取，请先查询原任务。")?;
        if observed["id"] != job
            || observed["request_id"] != receipt
            || observed["command"] != COMMAND
        {
            return Err("原搜索结果身份无法核对，未开始分析。".into());
        }
        if matches!(observed["status"].as_str(), Some("queued" | "running")) {
            return Err("GitHub 搜索还在进行，请等候选结果返回后再选择。".into());
        }
        if observed["status"] != "succeeded" {
            return Err("原搜索未返回可用候选，请先查询原任务。".into());
        }
        candidates(&observed["result"])
            .and_then(|list| list.get(ordinal - 1))
            .and_then(|row| row["repository_url"].as_str())
            .map(str::to_owned)
            .ok_or_else(|| "原搜索没有这个可核对的候选序号，请查询原结果。".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_project_names_bind_numbering_to_explicit_mentions_in_order() {
        let text = "项目名叫“Marked”。下一项仓库叫 owner/Tool。\n+            This project is called `HyperFrames`. 重复项目叫做 marked。另一个项目叫Vue.js，\n+            project named linked. https://github.com/org/linked";
        let links = video::repository_refs(text);
        assert_eq!(
            mentioned_names(text, &links),
            ["Marked", "owner/Tool", "HyperFrames", "Vue.js"]
        );
    }

    #[test]
    fn local_project_names_skip_urls_emails_incomplete_tokens_and_multiword_quotes() {
        let text = "项目叫 https://github.com/owner/repo。项目叫 build@example.org。项目叫 2026。\n+            项目名叫「Two Words」。Subproject called bogus. 仓库名是 name?token=hidden。\n+            仓库名叫 github.com/owner/repo。仓库名叫「Good-Name」。";
        assert_eq!(mentioned_names(text, &[]), ["Good-Name"]);
        let many = "项目叫 A1。项目叫 A2。项目叫 A3。项目叫 A4。项目叫 A5。项目叫 A6。";
        assert_eq!(mentioned_names(many, &[]), ["A1", "A2", "A3", "A4", "A5"]);
    }
}
