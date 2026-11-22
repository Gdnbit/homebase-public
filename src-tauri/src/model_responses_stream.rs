//! Validates typed OpenAI Responses events before returning a complete model round.
use super::{invalid, limit, Text};
use crate::assistant_protocol::{self, ModelReply};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Message,
    Reasoning,
    Tool,
}

struct ToolOutput {
    call_id: String,
    name: String,
    arguments: String,
    arguments_done: bool,
    item_done: bool,
}

struct Output {
    id: String,
    kind: Kind,
    parts: BTreeSet<u64>,
    tool: Option<ToolOutput>,
}

#[derive(Default)]
pub(super) struct Response {
    id: String,
    sequence: Option<u64>,
    outputs: BTreeMap<u64, Output>,
    output_ids: BTreeSet<String>,
    position: Option<(u64, u64)>,
    text: Text,
    allow_tools: bool,
}

impl Response {
    pub(super) fn new(allow_tools: bool) -> Self {
        Self { allow_tools, ..Self::default() }
    }

    pub(super) fn event(
        &mut self,
        name: &[u8],
        data: &[u8],
        publish: &mut impl FnMut(&str),
    ) -> Result<Option<ModelReply>, String> {
        let event: Value = serde_json::from_slice(data).map_err(|_| invalid())?;
        let event_type = event["type"].as_str().ok_or_else(invalid)?;
        let sequence = event["sequence_number"].as_u64().ok_or_else(invalid)?;
        if (!name.is_empty() && name != event_type.as_bytes())
            || self.sequence.is_some_and(|last| sequence <= last)
            || !event["error"].is_null()
        {
            return Err(invalid());
        }
        self.sequence = Some(sequence);

        let response = &event["response"];
        if event_type == "response.created" && self.id.is_empty() {
            self.id = checked_id(&response["id"])?.into();
            if !response["error"].is_null() || !response["incomplete_details"].is_null() {
                return Err(invalid());
            }
            return Ok(None);
        }
        if self.id.is_empty()
            || (!event["response_id"].is_null()
                && event["response_id"].as_str() != Some(self.id.as_str()))
        {
            return Err(invalid());
        }

        match event_type {
            "response.in_progress" | "response.queued" => self.check_response(response)?,
            "response.output_item.added" => self.add_output(&event)?,
            "response.output_item.done" => {
                let index = output_index(&event)?;
                let item = &event["item"];
                if let Some(tool) = &self.check_item(item, index)?.tool {
                    if !tool.arguments_done || tool.item_done || !tool.matches(item) {
                        return Err(invalid());
                    }
                    self.outputs.get_mut(&index).and_then(|output| output.tool.as_mut())
                        .ok_or_else(invalid)?.item_done = true;
                }
            }
            "response.function_call_arguments.delta" | "response.function_call_arguments.done" => {
                let tool = self.event_output_mut(&event)?.tool.as_mut().ok_or_else(invalid)?;
                if tool.arguments_done || tool.item_done {
                    return Err(invalid());
                }
                if event_type.ends_with(".done") {
                    if event["arguments"].as_str() != Some(tool.arguments.as_str())
                        || event.get("name").is_some_and(|name| name != &tool.name)
                    {
                        return Err(invalid());
                    }
                    tool.arguments_done = true;
                } else {
                    let delta = event["delta"].as_str().ok_or_else(invalid)?;
                    if tool.arguments.len() + delta.len() >= 16_000 {
                        return Err(limit());
                    }
                    tool.arguments.push_str(delta);
                }
            }
            "response.content_part.added" | "response.content_part.done" => {
                let output = self.event_output_mut(&event)?;
                match (output.kind, event["part"]["type"].as_str()) {
                    (Kind::Message, Some("output_text")) => {
                        let index = part_index(&event)?;
                        let valid = if event_type.ends_with(".added") {
                            output.parts.insert(index)
                        } else {
                            output.parts.contains(&index)
                        };
                        if !valid {
                            return Err(invalid());
                        }
                    }
                    (Kind::Reasoning, Some("reasoning_text")) => {}
                    _ => return Err(invalid()),
                }
            }
            "response.output_text.delta" => {
                let position = self.text_position(&event)?;
                if self.position.is_some_and(|last| position < last) {
                    return Err(invalid());
                }
                self.position = Some(position);
                let delta = event["delta"].as_str().ok_or_else(invalid)?;
                self.text.append(delta)?;
                if !delta.is_empty() {
                    publish(delta);
                }
            }
            "response.output_text.done" | "response.output_text.annotation.added" => {
                self.text_position(&event)?;
            }
            "response.reasoning_summary_part.added"
            | "response.reasoning_summary_part.done"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.delta"
            | "response.reasoning_text.done" => {
                if self.event_output(&event)?.kind != Kind::Reasoning {
                    return Err(invalid());
                }
            }
            "response.completed" => return self.completed(response).map(Some),
            // Includes refusals, unrequested hosted tools and failed or incomplete responses.
            _ => return Err(invalid()),
        }
        Ok(None)
    }

    fn add_output(&mut self, event: &Value) -> Result<(), String> {
        let index = output_index(event)?;
        let item = &event["item"];
        let id = checked_id(&item["id"])?;
        let kind = item_kind(item)?;
        if self.outputs.contains_key(&index) || !self.output_ids.insert(id.into()) {
            return Err(invalid());
        }
        let tool = if kind == Kind::Tool {
            if !self.allow_tools || self.outputs.values().filter(|output| output.tool.is_some()).count() >= 4 {
                return Err(invalid());
            }
            let call_id = checked_id(&item["call_id"])?;
            let name = checked_id(&item["name"])?;
            let arguments = item["arguments"].as_str().ok_or_else(invalid)?;
            if call_id.len() >= 200 || name.len() > 64 || arguments.len() >= 16_000
                || self.outputs.values().filter_map(|output| output.tool.as_ref()).any(|tool| tool.call_id == call_id)
            {
                return Err(invalid());
            }
            Some(ToolOutput { call_id: call_id.into(), name: name.into(), arguments: arguments.into(), arguments_done: false, item_done: false })
        } else {
            None
        };
        self.outputs.insert(
            index,
            Output {
                id: id.into(),
                kind,
                parts: BTreeSet::new(),
                tool,
            },
        );
        Ok(())
    }

    fn event_output(&self, event: &Value) -> Result<&Output, String> {
        self.outputs
            .get(&output_index(event)?)
            .filter(|output| event["item_id"].as_str() == Some(output.id.as_str()))
            .ok_or_else(invalid)
    }

    fn event_output_mut(&mut self, event: &Value) -> Result<&mut Output, String> {
        self.outputs
            .get_mut(&output_index(event)?)
            .filter(|output| event["item_id"].as_str() == Some(output.id.as_str()))
            .ok_or_else(invalid)
    }

    fn text_position(&self, event: &Value) -> Result<(u64, u64), String> {
        let output = self.event_output(event)?;
        let part = part_index(event)?;
        if output.kind != Kind::Message || !output.parts.contains(&part) {
            return Err(invalid());
        }
        Ok((output_index(event)?, part))
    }

    fn check_item(&self, item: &Value, index: u64) -> Result<&Output, String> {
        let kind = item_kind(item)?;
        self.outputs
            .get(&index)
            .filter(|output| item["id"].as_str() == Some(output.id.as_str()) && kind == output.kind)
            .ok_or_else(invalid)
    }

    fn check_response(&self, response: &Value) -> Result<(), String> {
        if response["id"].as_str() != Some(self.id.as_str())
            || !response["error"].is_null()
            || !response["incomplete_details"].is_null()
        {
            return Err(invalid());
        }
        Ok(())
    }

    fn completed(&self, response: &Value) -> Result<ModelReply, String> {
        self.check_response(response)?;
        let items = response["output"].as_array().ok_or_else(invalid)?;
        if response["status"] != "completed" || items.len() != self.outputs.len() {
            return Err(invalid());
        }
        let mut final_text = Text::default();
        for (index, item) in items.iter().enumerate() {
            let output = self.check_item(item, index as u64)?;
            if let Some(tool) = &output.tool {
                if !tool.item_done || !tool.matches(item) {
                    return Err(invalid());
                }
            }
            if output.kind == Kind::Message {
                let parts = item["content"].as_array().ok_or_else(invalid)?;
                if parts.len() != output.parts.len() {
                    return Err(invalid());
                }
                for (index, part) in parts.iter().enumerate() {
                    if part["type"] != "output_text" || !output.parts.contains(&(index as u64)) {
                        return Err(invalid());
                    }
                    final_text.append(part["text"].as_str().ok_or_else(invalid)?)?;
                }
            }
        }
        if !self.text.value.is_empty() && self.text.value != final_text.value {
            return Err(invalid());
        }
        if self.allow_tools {
            assistant_protocol::parse("responses", response).map_err(|_| invalid())
        } else {
            final_text.reply(&response["model"], &response["usage"])
        }
    }
}

impl ToolOutput {
    fn matches(&self, item: &Value) -> bool {
        item["call_id"] == self.call_id && item["name"] == self.name
            && item["arguments"] == self.arguments
            && item.get("status").is_none_or(|status| status == "completed")
    }
}

fn checked_id(value: &Value) -> Result<&str, String> {
    value
        .as_str()
        .filter(|id| !id.trim().is_empty() && id.len() <= 256)
        .ok_or_else(invalid)
}

fn item_kind(item: &Value) -> Result<Kind, String> {
    match item["type"].as_str() {
        Some("message") if item["role"] == "assistant" => Ok(Kind::Message),
        Some("reasoning") => Ok(Kind::Reasoning),
        Some("function_call") => Ok(Kind::Tool),
        _ => Err(invalid()),
    }
}

fn output_index(event: &Value) -> Result<u64, String> {
    event["output_index"].as_u64().ok_or_else(invalid)
}

fn part_index(event: &Value) -> Result<u64, String> {
    event["content_index"].as_u64().ok_or_else(invalid)
}

#[cfg(test)]
pub(crate) mod test_frames {
    use serde_json::{json, Value};

    pub(crate) fn frame(event: &Value) -> String {
        format!(
            "event: {}\r\ndata: {}\r\n\r\n",
            event["type"].as_str().unwrap(),
            event
        )
    }

    pub(crate) fn prefix() -> [Value; 6] {
        [
            json!({"type":"response.created","sequence_number":1,"response":{"id":"resp_lab","status":"in_progress","output":[]}}),
            json!({"type":"response.output_item.added","sequence_number":2,"output_index":0,"item":{"id":"reason_lab","type":"reasoning","summary":[]}}),
            json!({"type":"response.reasoning_summary_text.delta","sequence_number":3,"output_index":0,"item_id":"reason_lab","summary_index":0,"delta":"private-reasoning"}),
            json!({"type":"response.output_item.added","sequence_number":4,"output_index":1,"item":{"id":"message_lab","type":"message","role":"assistant","status":"in_progress","content":[]}}),
            json!({"type":"response.content_part.added","sequence_number":5,"output_index":1,"item_id":"message_lab","content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
            delta(6, "合成前缀"),
        ]
    }

    pub(crate) fn delta(sequence: u64, text: &str) -> Value {
        json!({"type":"response.output_text.delta","sequence_number":sequence,"output_index":1,"item_id":"message_lab","content_index":0,"delta":text})
    }

    pub(crate) fn completed(sequence: u64, text: &str) -> Value {
        json!({"type":"response.completed","sequence_number":sequence,"response":{
            "id":"resp_lab","model":"synthetic","status":"completed","error":null,
            "usage":{"input_tokens":9,"output_tokens":5},"output":[
                {"type":"reasoning","id":"reason_lab","summary":[{"type":"summary_text","text":"private-reasoning"}]},
                {"type":"message","role":"assistant","id":"message_lab","status":"completed","content":[{"type":"output_text","text":text,"annotations":[]}]}
            ]
        }})
    }

    pub(crate) fn tool_round() -> Vec<Value> {
        let call = json!({"id":"item_tool","type":"function_call","call_id":"original-tool-call",
            "name":"homebase_memory","status":"completed",
            "arguments":"{\"action\":\"remember\",\"revision\":0,\"title\":\"合成偏好\",\"content\":\"蓝点\"}"});
        let mut events = prefix().to_vec();
        events[5]["delta"] = json!("正在核对合成偏好。");
        let mut active = call.clone();
        active["arguments"] = json!("");
        active["status"] = json!("in_progress");
        events.extend([
            json!({"type":"response.output_item.added","sequence_number":7,"output_index":2,"item":active}),
            json!({"type":"response.function_call_arguments.delta","sequence_number":8,"output_index":2,"item_id":"item_tool","delta":"{\"action\":\"remember\",\"revision\":0,\"title\":\"合成偏好\",\"content\":\""}),
            json!({"type":"response.function_call_arguments.delta","sequence_number":9,"output_index":2,"item_id":"item_tool","delta":"蓝点\"}"}),
            json!({"type":"response.function_call_arguments.done","sequence_number":10,"output_index":2,"item_id":"item_tool","name":"homebase_memory","arguments":call["arguments"]}),
            json!({"type":"response.output_item.done","sequence_number":11,"output_index":2,"item":call}),
        ]);
        let mut done = completed(12, "正在核对合成偏好。");
        done["response"]["output"][0]["encrypted_content"] = json!("synthetic-encrypted-context");
        done["response"]["output"].as_array_mut().unwrap().push(call);
        events.push(done);
        events
    }
}

#[cfg(test)]
mod tests {
    use super::test_frames::*;
    use super::*;
    use crate::model_capability_io::dialogue_stream::{
        Failure, Kind as FailureKind, Protocol, Sse,
    };
    use crate::{assistant_protocol, model_capabilities, model_capability_io};
    use serde_json::json;

    #[test]
    fn openai_text_plan_remains_bound_to_the_explicit_route_and_free_policy() {
        let routes = model_capability_io::discover(
            "openai",
            &json!({"data":[
                {"id":"gpt-4o-mini"}, {"id":"gpt-5-mini"}, {"id":"gpt-5.4-mini"},
                {"id":"gpt-unknown"}, {"id":"text-embedding-3-small"}
            ]}),
            "https://api.openai.com/v1/models",
        )
        .unwrap();
        let route = routes
            .iter()
            .find(|route| route.model_id == "gpt-4o-mini")
            .unwrap();
        let embedding = routes
            .iter()
            .find(|route| route.model_id == "text-embedding-3-small")
            .unwrap();
        let mut request = assistant_protocol::ModelRequest {
            vision: None,
            route_id: route.route_id.clone(),
            messages: vec![json!({"role":"user","content":"合成问题"})],
            max_tokens: 1200,
            credential_source: Default::default(),
            free_only: false,
            definitions: Some(vec![]),
        };
        assert!(Protocol::for_route(embedding, &request).is_none());
        let protocol = Protocol::for_route(route, &request).unwrap();
        assert!(matches!(protocol, Protocol::Responses));
        let input =
            assistant_protocol::body(&route.protocol, &route.provider_id, &request).unwrap();
        let (url, mut body) = model_capabilities::prepare(route, &input).unwrap();
        assert_eq!(body["stream"], false);
        protocol.enable(&route.provider_id, &mut body);
        assert_eq!(url, "https://api.openai.com/v1/responses");
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(body["max_output_tokens"], 1200);
        for key in ["tools", "tool_choice", "include"] {
            assert!(body.get(key).is_none());
        }
        request.definitions = Some(assistant_protocol::tools());
        for id in ["gpt-4o-mini", "gpt-5-mini", "gpt-5.4-mini"] {
            let route = routes.iter().find(|r| r.model_id == id).unwrap();
            request.route_id = route.route_id.clone();
            let protocol = Protocol::for_route(route, &request).unwrap();
            assert!(matches!(protocol, Protocol::ResponsesTools));
            let input = assistant_protocol::body("responses", "openai", &request).unwrap();
            let (url, mut body) = model_capabilities::prepare(route, &input).unwrap();
            protocol.enable("openai", &mut body);
            assert_eq!(url, "https://api.openai.com/v1/responses");
            assert_eq!(body["store"], false);
            assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
            assert!(!body["tools"].as_array().unwrap().is_empty());
        }
        let unspecified = routes.iter().find(|r| r.model_id == "gpt-unknown").unwrap();
        request.route_id = unspecified.route_id.clone();
        assert!(Protocol::for_route(unspecified, &request).is_none());
        request.free_only = true;
        assert!(assistant_protocol::body(&route.protocol, &route.provider_id, &request).is_err());
        request.definitions = None;
        assert!(Protocol::for_route(route, &request).is_none());
    }

    #[test]
    fn responses_tools_require_matching_deltas_item_and_completed_response() {
        let events = tool_round();
        let mut reader = Response::new(true);
        let mut visible = Vec::new();
        let mut publish = |text: &str| visible.push(text.to_string());
        for event in &events[..events.len() - 1] {
            assert!(reader.event(b"", event.to_string().as_bytes(), &mut publish).unwrap().is_none());
        }
        let result = reader.event(b"", events.last().unwrap().to_string().as_bytes(), &mut publish).unwrap().unwrap();
        assert_eq!(visible, ["正在核对合成偏好。"]);
        assert_eq!(result.calls[0].id, "original-tool-call");
        assert_eq!(result.calls[0].arguments["content"], "蓝点");
        assert_eq!(result.message["response_items"][0]["encrypted_content"], "synthetic-encrypted-context");

        for (index, change) in [
            (7, ("item_id", json!("foreign-item"))),
            (8, ("delta", json!("missing text"))),
            (9, ("arguments", json!("{}"))),
            (10, ("item", json!({"type":"function_call","id":"item_tool","name":"wrong","arguments":"{}"}))),
            (11, ("response", completed(12, "foreign")["response"].clone())),
        ] {
            let mut invalid = events.clone();
            invalid[index][change.0] = change.1;
            let mut response = Response::new(true);
            let mut published_final = false;
            let result = invalid.iter().try_for_each(|item| response.event(b"", item.to_string().as_bytes(), &mut |_| {}).map(|reply| {
                published_final |= reply.is_some();
            }));
            assert!(result.is_err());
            assert!(!published_final);
        }
    }

    #[test]
    fn interleaved_tool_arguments_stay_attached_to_their_call() {
        let mut events = tool_round();
        let mut second = json!({"type":"function_call","id":"item_other","call_id":"call_other",
            "name":"homebase_catalog","arguments":"","status":"in_progress"});
        events.insert(8, json!({"type":"response.output_item.added","output_index":3,"item":second}));
        events.insert(9, json!({"type":"response.function_call_arguments.delta","output_index":3,
            "item_id":"item_other","delta":"{"}));
        events.insert(11, json!({"type":"response.function_call_arguments.delta","output_index":3,
            "item_id":"item_other","delta":"}"}));
        events.insert(12, json!({"type":"response.function_call_arguments.done","output_index":3,
            "item_id":"item_other","arguments":"{}"}));
        second["arguments"] = json!("{}");
        second["status"] = json!("completed");
        events.insert(13, json!({"type":"response.output_item.done","output_index":3,"item":second}));
        events.last_mut().unwrap()["response"]["output"].as_array_mut().unwrap().push(second);
        for (position, event) in events.iter_mut().enumerate() {
            event["sequence_number"] = json!(position + 1);
        }
        let mut reader = Response::new(true);
        for event in &events[..events.len() - 1] {
            assert!(reader.event(b"", event.to_string().as_bytes(), &mut |_| {}).unwrap().is_none());
        }
        let mut foreign = events.last().unwrap().clone();
        foreign["response"]["output"][2]["arguments"] = json!("{}");
        let reply = reader.event(b"", events.last().unwrap().to_string().as_bytes(), &mut |_| {}).unwrap().unwrap();
        assert_eq!(reply.calls.len(), 2);
        assert_eq!(reply.calls[0].id, "original-tool-call");
        assert_eq!(reply.calls[0].arguments["content"], "蓝点");
        assert_eq!(reply.calls[1].id, "call_other");
        assert_eq!(reply.calls[1].arguments, json!({}));

        let mut reader = Response::new(true);
        for event in &events[..events.len() - 1] {
            reader.event(b"", event.to_string().as_bytes(), &mut |_| {}).unwrap();
        }
        assert!(reader.event(b"", foreign.to_string().as_bytes(), &mut |_| {}).is_err());
    }

    #[test]
    fn typed_completion_checks_text_and_preserves_only_visible_output_and_usage() {
        let mut events = prefix().to_vec();
        events.extend([
            delta(8, "🙂"),
            json!({"type":"response.output_text.done","sequence_number":9,"output_index":1,"item_id":"message_lab","content_index":0,"text":"合成前缀🙂"}),
            completed(10, "合成前缀🙂"),
        ]);
        let mut decoder = Response::default();
        let mut wire = Sse::default();
        let mut visible = Vec::new();
        let mut result = None;
        for event in events {
            for byte in frame(&event).bytes() {
                let reply = wire
                    .push(&[byte], |name, data| {
                        decoder.event(name, data, &mut |text| visible.push(text.to_string()))
                    })
                    .unwrap();
                if reply.is_some() {
                    assert_eq!(event["type"], "response.completed");
                    result = reply;
                }
            }
        }
        let reply = result.unwrap();
        assert_eq!(visible, ["合成前缀", "🙂"]);
        assert_eq!(reply.text, "合成前缀🙂");
        assert!(reply.calls.is_empty());
        assert_eq!(
            reply.message,
            json!({"role":"assistant","content":"合成前缀🙂"})
        );
        let usage = reply.usage.unwrap();
        assert_eq!(usage.prompt_tokens, Some(9));
        assert_eq!(usage.completion_tokens, Some(5));
        assert_eq!(usage.model.as_deref(), Some("synthetic"));
    }

    #[test]
    fn rejects_foreign_or_incomplete_frames_without_publishing_their_data() {
        let mut foreign_item = delta(7, "untrusted");
        foreign_item["item_id"] = json!("foreign");
        let mut foreign_response = completed(7, "合成前缀");
        foreign_response["response"]["id"] = json!("foreign");
        let mut missing_text = completed(7, "");
        missing_text["response"]["output"][1]["content"] = json!([]);
        let mut unsafe_final = completed(7, "合成前缀");
        unsafe_final["response"]["output"][0]["type"] = json!("function_call");
        for event in [
            foreign_item,
            foreign_response,
            missing_text,
            unsafe_final,
            delta(6, "untrusted"),
            completed(7, "untrusted"),
            json!({"type":"error","sequence_number":7,"message":"untrusted"}),
            json!({"type":"response.incomplete","sequence_number":7}),
            json!({"type":"response.failed","sequence_number":7}),
            json!({"type":"response.refusal.delta","sequence_number":7,"delta":"untrusted"}),
            json!({"type":"response.function_call_arguments.delta","sequence_number":7,"delta":"untrusted"}),
        ] {
            let mut decoder = Response::default();
            for initial in prefix() {
                assert!(decoder
                    .event(b"", initial.to_string().as_bytes(), &mut |_| {})
                    .unwrap()
                    .is_none());
            }
            let err = decoder
                .event(b"", event.to_string().as_bytes(), &mut |_| {
                    panic!("untrusted frame emitted")
                })
                .err()
                .unwrap();
            assert!(matches!(
                Failure::decode(&err).unwrap().kind,
                FailureKind::InvalidResponse
            ));
            assert!(!err.contains("untrusted"));
        }
        let mut decoder = Response::default();
        assert!(decoder
            .event(
                b"response.completed",
                prefix()[0].to_string().as_bytes(),
                &mut |_| {}
            )
            .is_err());
        assert!(decoder.event(b"", b"[DONE]", &mut |_| {}).is_err());
    }
}
