//! Chat Completions text and fully assembled tool calls over SSE.
use super::{invalid, limit, Text};
use crate::assistant_protocol::{self, ModelReply};
use serde_json::{json, Value};
use std::collections::BTreeMap;

struct ToolFragments {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Default)]
pub(super) struct Response {
    text: Text,
    model: Value,
    usage: Value,
    finish_reason: Option<&'static str>,
    allow_tools: bool,
    response_id: Option<String>,
    reasoning: Option<String>,
    calls: BTreeMap<usize, ToolFragments>,
}

impl Response {
    pub(super) fn new(allow_tools: bool) -> Self {
        Self { allow_tools, ..Self::default() }
    }

    pub(super) fn event(
        &mut self,
        data: &[u8],
        publish: &mut impl FnMut(&str),
    ) -> Result<Option<ModelReply>, String> {
        if data == b"[DONE]" {
            return self.reply().map(Some);
        }
        self.apply(data, publish)?;
        Ok(None)
    }

    fn reply(&self) -> Result<ModelReply, String> {
        let reason = self.finish_reason.ok_or_else(invalid)?;
        if !self.allow_tools {
            return self.text.reply(&self.model, &self.usage);
        }
        let mut message = json!({"role":"assistant","content":self.text.value});
        if let Some(reasoning) = &self.reasoning {
            message["reasoning_content"] = json!(reasoning);
        }
        if !self.calls.is_empty() {
            if self.calls.keys().copied().ne(0..self.calls.len()) {
                return Err(invalid());
            }
            message["tool_calls"] = json!(self.calls.values().map(|call| json!({
                "id":call.id, "type":"function", "function":{"name":call.name,"arguments":call.arguments},
            })).collect::<Vec<_>>());
        }
        assistant_protocol::parse("chat", &json!({"model":self.model,"usage":self.usage,"choices":[
            {"finish_reason":reason,"message":message}
        ]})).map_err(|_| invalid())
    }

    fn add_tools(&mut self, delta: &Value) -> Result<(), String> {
        let chunks = delta.as_array().ok_or_else(invalid)?;
        if chunks.len() > 4 || !self.allow_tools && !chunks.is_empty() {
            return Err(invalid());
        }
        let mut seen = [false; 4];
        for chunk in chunks {
            let index = chunk["index"].as_u64().filter(|index| *index < 4).ok_or_else(invalid)? as usize;
            if std::mem::replace(&mut seen[index], true) || !chunk["function"].is_object() {
                return Err(invalid());
            }
            let function = &chunk["function"];
            if let std::collections::btree_map::Entry::Vacant(entry) = self.calls.entry(index) {
                let id = chunk["id"].as_str().filter(|id| !id.is_empty() && id.len() < 200).ok_or_else(invalid)?;
                let name = function["name"].as_str().filter(|name| !name.is_empty() && name.len() <= 64).ok_or_else(invalid)?;
                if chunk["type"] != "function" { return Err(invalid()); }
                entry.insert(ToolFragments { id: id.into(), name: name.into(), arguments: String::new() });
            } else if !chunk["id"].is_null() || !chunk["type"].is_null() || !function["name"].is_null() {
                return Err(invalid());
            }
            if !function["arguments"].is_null() {
                let part = function["arguments"].as_str().ok_or_else(invalid)?;
                let arguments = &mut self.calls.get_mut(&index).ok_or_else(invalid)?.arguments;
                if arguments.len() + part.len() >= 16_000 { return Err(limit()); }
                arguments.push_str(part);
            }
        }
        Ok(())
    }

    fn apply(&mut self, frame: &[u8], publish: &mut impl FnMut(&str)) -> Result<(), String> {
        let value: Value = serde_json::from_slice(frame).map_err(|_| invalid())?;
        if !value["error"].is_null() {
            return Err(invalid());
        }
        if self.allow_tools {
            let id = value["id"].as_str().filter(|id| !id.is_empty() && id.len() < 200).ok_or_else(invalid)?;
            if self.response_id.as_deref().is_some_and(|previous| previous != id) {
                return Err(invalid());
            }
            self.response_id.get_or_insert_with(|| id.into());
        }
        if value["model"].is_string() {
            self.model = value["model"].clone();
        }
        if value["usage"].is_object() {
            self.usage = value["usage"].clone();
        }
        let choices = value["choices"].as_array().ok_or_else(invalid)?;
        let choice = match choices.as_slice() {
            [] => return Ok(()),
            [choice] if self.finish_reason.is_none() && choice["index"].as_u64() == Some(0) => choice,
            _ => return Err(invalid()),
        };
        let delta = &choice["delta"];
        if !delta.is_object()
            || (!delta["role"].is_null() && delta["role"] != "assistant")
            || !delta["function_call"].is_null()
            || self.allow_tools && !delta["refusal"].is_null()
        {
            return Err(invalid());
        }
        if !delta["tool_calls"].is_null() {
            self.add_tools(&delta["tool_calls"])?;
        }
        if self.allow_tools && !delta["reasoning_content"].is_null() {
            let part = delta["reasoning_content"].as_str().ok_or_else(invalid)?;
            let reasoning = self.reasoning.get_or_insert_with(String::new);
            if reasoning.len() + part.len() > assistant_protocol::MAX_REPLY_BYTES { return Err(limit()); }
            reasoning.push_str(part);
        }
        if !choice["finish_reason"].is_null() {
            self.finish_reason = Some(match choice["finish_reason"].as_str() {
                Some("stop") if self.calls.is_empty() => "stop",
                Some("tool_calls") if !self.calls.is_empty() => "tool_calls",
                _ => return Err(invalid()),
            });
        }
        if !delta["content"].is_null() {
            let text = delta["content"].as_str().ok_or_else(invalid)?;
            self.text.append(text)?;
            if !text.is_empty() {
                publish(text);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Failure, Kind, Protocol, Sse, MAX_EVENT};
    use super::*;
    use crate::assistant_protocol;
    use serde_json::json;

    #[test]
    fn official_deepseek_chat_routes_keep_fixed_hosts_and_free_mode_refuses_them() {
        for model in ["deepseek-flash", "deepseek-v4-pro"] {
            let route = crate::model_capabilities::seed_models()
                .into_iter()
                .find(|route| route.model_id == model && route.provider_id == "deepseek")
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
            let input = assistant_protocol::body(&route.protocol, "deepseek", &request).unwrap();
            let (url, mut body) = crate::model_capabilities::prepare(&route, &input).unwrap();
            Protocol::for_route(&route, &request)
                .unwrap()
                .enable("deepseek", &mut body);
            assert_eq!(url, "https://api.deepseek.com/chat/completions");
            assert_eq!(body["model"], model);
            assert_eq!(body["stream"], true);
            assert_eq!(body["thinking"]["type"], "disabled");
            assert!(body.get("tools").is_none());
            assert_eq!(route.account_state, "not_verified");
            request.definitions = Some(assistant_protocol::tools());
            let protocol = Protocol::for_route(&route, &request).unwrap();
            assert!(matches!(protocol, Protocol::ChatTools));
            let input = assistant_protocol::body(&route.protocol, "deepseek", &request).unwrap();
            let (_, mut body) = crate::model_capabilities::prepare(&route, &input).unwrap();
            protocol.enable("deepseek", &mut body);
            assert_eq!(body["stream_options"]["include_usage"], true);
            assert!(body.get("thinking").is_none());
            assert!(!body["tools"].as_array().unwrap().is_empty());
            request.free_only = true;
            assert!(assistant_protocol::body(&route.protocol, "deepseek", &request).is_err());
        }
    }

    #[test]
    fn streamed_tools_keep_whole_arguments_and_reasoning_private_until_done() {
        let mut reader = Response::new(true);
        let mut texts = Vec::new();
        let mut publish = |text: &str| texts.push(text.to_string());
        for (delta, finish) in [
            (json!({"reasoning_content":"synthetic-private", "content":"核对", "tool_calls":[
                {"index":0,"id":"call-a","type":"function","function":{"name":"homebase_memory","arguments":"{\"title\":\""}},
                {"index":1,"id":"call-b","type":"function","function":{"name":"homebase_list","arguments":"{}"}}
            ]}), Value::Null),
            (json!({"tool_calls":[{"index":0,"function":{"arguments":"合成\"}"}}]}),json!("tool_calls")),
        ] {
            let frame = json!({"id":"synthetic-response","choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
            assert!(reader.event(frame.to_string().as_bytes(), &mut publish).unwrap().is_none());
        }
        let result = reader.event(b"[DONE]", &mut publish).unwrap().unwrap();
        assert_eq!(texts, ["核对"]);
        assert_eq!(result.calls[0].id, "call-a");
        assert_eq!(result.calls[0].arguments, json!({"title":"合成"}));
        assert_eq!(result.calls[1].id, "call-b");
        assert_eq!(result.message["reasoning_content"], "synthetic-private");
    }

    #[test]
    fn invalid_streamed_tool_sequences_never_return_a_call() {
        let tool = |index, id, args| json!({"index":index,"id":id,"type":"function","function":{"name":"synthetic","arguments":args}});
        let event = |id, chunks: Value, finish| json!({"id":id,"choices":[{"index":0,"delta":{"tool_calls":chunks},"finish_reason":finish}]}).to_string();
        for frames in [
            vec![event("response",json!([tool(1,"a","{}")]),"tool_calls")],
            vec![event("response",json!([tool(0,"a","{")]),"tool_calls")],
            vec![event("response",json!([tool(0,"a","{}")]),"stop")],
            vec![event("response",json!([tool(0,"a","{}"),tool(1,"a","{}")]),"tool_calls")],
            vec![event("response",json!([tool(0,"a","{}"),tool(0,"a","{}")]),"tool_calls")],
            vec![event("response",json!([tool(0,"a","{}")]),"tool_calls"),event("other",json!([]),"stop")],
        ] {
            let mut reader = Response::new(true);
            let mut publish = |_: &str| panic!("an invalid batch emitted text");
            let outcome = frames.iter().try_for_each(|frame| reader.event(frame.as_bytes(), &mut publish).map(|_| ()))
                .and_then(|_| reader.event(b"[DONE]", &mut publish));
            assert!(outcome.is_err());
        }
    }

    #[test]
    fn split_frames_emit_only_visible_text_and_apply_final_usage() {
        let stream = concat!(
            ": OPENROUTER PROCESSING\r\n\r\n",
            "data: {\"model\":\"synthetic\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"hidden\",\"content\":\"合成\"},\"finish_reason\":null}]}\r\n\r\n",
            "data: {\"choices\":[{\n",
            "data: \"index\":0,\"delta\":{\"content\":\"🙂\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"cost\":0,\"prompt_tokens\":7,\"completion_tokens\":3}}\n\n",
            "data: [DONE]\n\n",
        );
        let mut decoder = Response::default();
        let mut visible = Vec::new();
        let mut reply = None;
        let mut sse = Sse::default();
        for byte in stream.bytes() {
            let result = sse
                .push(&[byte], |_, data| {
                    decoder.event(data, &mut |text| visible.push(text.to_string()))
                })
                .unwrap();
            if result.is_some() {
                reply = result;
            }
        }
        assert_eq!(visible, ["合成", "🙂"]);
        let reply = reply.unwrap();
        assert_eq!(reply.text, "合成🙂");
        assert_eq!(reply.usage.unwrap().completion_tokens, Some(3));
        assert!(reply.calls.is_empty());
        assert!(!reply.message.to_string().contains("hidden"));
    }

    #[test]
    fn rejects_incomplete_terminal_error_and_unexpected_tools() {
        for invalid_frame in [
            "data: [DONE]\n\n",
            "data: {\"error\":{\"message\":\"synthetic-private-error\"}}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{}]}}]}\n\n",
        ] {
            let mut decoder = Response::default();
            let err = Sse::default()
                .push(invalid_frame.as_bytes(), |_, data| {
                    decoder.event(data, &mut |_| panic!("invalid data emitted"))
                })
                .err()
                .unwrap();
            assert!(!err.contains("synthetic-private-error"));
            assert!(matches!(
                Failure::decode(&err).unwrap().kind,
                Kind::InvalidResponse
            ));
        }
        assert!(Sse::default()
            .push(&vec![b'x'; MAX_EVENT + 1], |_, _| Ok(None))
            .is_err());
    }
}
