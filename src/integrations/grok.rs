//! xAI (Grok) API で校正・翻訳・質問応答を行うクライアント。
//! 校正・翻訳は chat completions、質問は Responses API (Agent Tools:
//! web_search / x_search) を使う。旧 Live Search (search_parameters) は
//! 2026-01 に廃止された。
//! eliza-agent-server (https://github.com/cympfh/eliza-agent-server) の
//! TranslatorAgent / QuestionAgent を移植したもの。memory は持ち込まない。

use serde::Serialize;

use crate::config::TranslateModel;

const XAI_CHAT_URL: &str = "https://api.x.ai/v1/chat/completions";
/// 質問モード用。サーバーサイド検索 (web_search/x_search) は
/// Responses API (Agent Tools) でしか使えない。旧 Live Search (search_parameters) は廃止済み
const XAI_RESPONSES_URL: &str = "https://api.x.ai/v1/responses";
/// 校正・質問のモデル。翻訳のモデルは設定 (TranslateModel) で選ぶ
const MODEL: &str = "grok-4.7";
/// grok-4.7 は reasoning を無効化できないので最小の low を指定する
const REASONING_EFFORT: &str = "low";

/// 校正: フィラー除去と言い間違いの修正だけ。内容・言語は変えない
const PROOFREAD_PROMPT: &str = "\
あなたは音声認識の結果を整える校正者です。
入力は音声認識の書き起こしで、フィラーや言い間違いを含みます。

やること:
- フィラー(えーと、あのー、そのー、うーん、まあ 等)を取り除く
- 言い間違い・言い淀み(「明日の…明後日の会議」→「明後日の会議」)を後の発言に修正する
- 音声認識の明らかな誤変換を文脈から直す

やらないこと:
- 内容の追加・要約・言い換え
- 言語の変換(翻訳しない)
- 文体の変更(話し言葉は話し言葉のまま)

校正結果のみを返してください。前置き・説明は一切不要です。";

/// 翻訳。翻訳元言語は指定しない(自動判定)。
/// 以前の few-shot 付きのプロンプトは、grok-4.20 non-reasoning で訳文の頭に
/// 「结果：」が付くことがあった。この 1 文にしても 4 モデルとも訳の質は落ちず、
/// 質問や指示の文も答えずに訳した(issue #20)
const TRANSLATE_PROMPT: &str = "入力を{target_lang}に翻訳し、訳文だけを出力する。";

/// 質問: eliza-agent-server の eliza/prompt/ELIZA.md から話し方の指針だけ移植
/// (agent_name=question, context=vrchat)。ペルソナ・memory は持ち込まない
const QUESTION_PROMPT: &str = "\
<role>
あなたはユーザーのパーソナルアシスタントとして、日常的なタスクをサポートします。
</role>

<話し方>
言語：日本語
発言から無駄を消せ。削除対象：

- 敬語・丁寧語（です/ます/ございます → 体言止め・用言止め）
- クッション言葉（えーと/まあ/ちなみに/一応/基本的に）
- ぼかし（〜かもしれません/〜と思われます/おそらく）
- 冗長助詞（〜することができる → 〜できる）
- 冗長接続（〜ということになりますので → だから）
- 自明な助詞（が/の/を/に/で/は/と/も） - 意味通じるなら省略
- 情報水増し - 聞かれたことだけ答える
</話し方>

<environment>
あなたとユーザーは VRChat の中にいます。
ユーザーは VRChat の中から speech-to-text であなたに話しかけています。
音声認識の誤りがあるかもしれませんが、ユーザーの発言はあたかも音声であるかのように理解し、ユーザーの意図を汲み取る必要があります。

あなたからユーザーへの最終的な返答は、テキストチャットで行います。
100文字から140文字程度の短文で返答してください。
</environment>

<instruction>
web_search, x_search を使って、ユーザーの質問に答えることが求められています。
自分の知識は使わず、web/X検索を積極的に利用せよ。

出力は構造化フォーマットに従うこと:
- context: 回答の根拠・補足など、ユーザーに見せないメモ
- citation: 参考にした URL の一覧
- answer: ユーザーへの最終回答文だけ。URL・出典表記・前置きを含めない
</instruction>";

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    /// None なら送らない(grok-4.20 non-reasoning)
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<&'a str>,
    messages: Vec<ChatMessage<'a>>,
}

#[derive(Serialize)]
struct Reasoning<'a> {
    effort: &'a str,
}

#[derive(Serialize)]
struct BuiltinTool<'a> {
    #[serde(rename = "type")]
    tool_type: &'a str,
}

#[derive(Serialize)]
struct JsonSchemaFormat<'a> {
    #[serde(rename = "type")]
    format_type: &'a str,
    name: &'a str,
    strict: bool,
    schema: serde_json::Value,
}

#[derive(Serialize)]
struct TextFormat<'a> {
    format: JsonSchemaFormat<'a>,
}

/// 質問モードの構造化出力スキーマ。context/citation はモデルに書き出させるが
/// アプリ側では捨てる。answer にはユーザーへの回答文だけが入る
fn question_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "context": {"type": "string"},
            "citation": {"type": "string"},
            "answer": {"type": "string"},
        },
        "required": ["context", "citation", "answer"],
        "additionalProperties": false,
    })
}

/// Responses API (Agent Tools) のリクエスト。質問モード専用
#[derive(Serialize)]
struct ResponsesRequest<'a> {
    model: &'a str,
    reasoning: Reasoning<'a>,
    input: Vec<ChatMessage<'a>>,
    tools: Vec<BuiltinTool<'a>>,
    text: TextFormat<'a>,
}

pub struct GrokClient {
    api_key: String,
}

impl GrokClient {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    /// 校正: フィラー除去・言い間違い修正のみ。内容は変えない
    pub fn correct(&self, text: &str) -> Result<String, String> {
        self.chat(chat_request(
            MODEL,
            Some(REASONING_EFFORT),
            PROOFREAD_PROMPT,
            text,
        ))
    }

    /// 翻訳: 翻訳元は自動判定、翻訳先は target_lang (「英語」などの表示名)。
    /// モデルと reasoning の強さは model で決まる
    pub fn translate(
        &self,
        model: TranslateModel,
        target_lang: &str,
        text: &str,
    ) -> Result<String, String> {
        let prompt = translate_prompt(target_lang);
        self.chat(translate_request(model, &prompt, text))
    }

    /// 質問: Responses API のサーバーサイドツール (web_search / x_search /
    /// code_execution) を使って答える。eliza-agent-server の QuestionAgent と
    /// 同じく、現在日時 (JST) を system メッセージで添える
    pub fn question(&self, text: &str) -> Result<String, String> {
        let now =
            chrono::Utc::now().with_timezone(&chrono::FixedOffset::east_opt(9 * 3600).unwrap());
        let now_message = format!("現在の日時（JST）: {}", now.format("%Y-%m-%d %H:%M:%S"));
        let request = ResponsesRequest {
            model: MODEL,
            reasoning: Reasoning {
                effort: REASONING_EFFORT,
            },
            input: vec![
                ChatMessage {
                    role: "system",
                    content: QUESTION_PROMPT,
                },
                ChatMessage {
                    role: "system",
                    content: &now_message,
                },
                ChatMessage {
                    role: "user",
                    content: text,
                },
            ],
            tools: vec![
                BuiltinTool {
                    tool_type: "web_search",
                },
                BuiltinTool {
                    tool_type: "x_search",
                },
                BuiltinTool {
                    tool_type: "code_execution",
                },
            ],
            text: TextFormat {
                format: JsonSchemaFormat {
                    format_type: "json_schema",
                    name: "question_answer",
                    strict: true,
                    schema: question_schema(),
                },
            },
        };

        // 検索が走ると時間がかかるので長めに待つ
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()
            .unwrap_or_default();

        let raw = client
            .post(XAI_RESPONSES_URL)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .map_err(|e| format!("Failed to send to xAI: {}", e))?
            .text()
            .map_err(|e| format!("Failed to read xAI response: {}", e))?;

        let body: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("Failed to parse xAI response: {}. Body was: {}", e, raw))?;

        // output[] から message アイテムの output_text を集める
        let mut texts: Vec<&str> = Vec::new();
        if let Some(items) = body["output"].as_array() {
            for item in items {
                if item["type"].as_str() != Some("message") {
                    continue;
                }
                if let Some(parts) = item["content"].as_array() {
                    for part in parts {
                        if part["type"].as_str() == Some("output_text")
                            && let Some(t) = part["text"].as_str()
                        {
                            texts.push(t);
                        }
                    }
                }
            }
        }
        let joined = texts.join("\n");
        let joined = joined.trim();
        if !joined.is_empty() {
            // 構造化出力の JSON から answer だけ取り出す。context/citation は捨てる
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(joined)
                && let Some(answer) = v["answer"].as_str()
                && !answer.trim().is_empty()
            {
                return Ok(answer.trim().to_string());
            }
            return Ok(joined.to_string());
        }
        // 念のためトップレベルの output_text も見る
        body["output_text"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("xAI response had no content. Body was: {}", raw))
    }

    fn chat(&self, request: ChatRequest<'_>) -> Result<String, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();

        let response = client
            .post(XAI_CHAT_URL)
            .bearer_auth(&self.api_key)
            .json(&request)
            .send()
            .map_err(|e| format!("Failed to send to xAI: {}", e))?;

        let raw = response
            .text()
            .map_err(|e| format!("Failed to read xAI response: {}", e))?;

        let body: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("Failed to parse xAI response: {}. Body was: {}", e, raw))?;

        body["choices"][0]["message"]["content"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("xAI response had no content. Body was: {}", raw))
    }
}

fn translate_prompt(target_lang: &str) -> String {
    TRANSLATE_PROMPT.replace("{target_lang}", target_lang)
}

fn chat_request<'a>(
    model: &'a str,
    reasoning_effort: Option<&'a str>,
    system: &'a str,
    user: &'a str,
) -> ChatRequest<'a> {
    ChatRequest {
        model,
        reasoning_effort,
        messages: vec![
            ChatMessage {
                role: "system",
                content: system,
            },
            ChatMessage {
                role: "user",
                content: user,
            },
        ],
    }
}

/// 翻訳のリクエスト。モデル ID と reasoning_effort は TranslateModel から決める
fn translate_request<'a>(model: TranslateModel, prompt: &'a str, text: &'a str) -> ChatRequest<'a> {
    chat_request(model.model_id(), model.reasoning_effort(), prompt, text)
}

/// 出力フォーマットのテンプレートを埋める。
/// 使えるプレースホルダ(ASCII のみ): `{source}` `{translated}` `{answer}`。
/// 知らないプレースホルダはそのまま残す
pub fn render_template(template: &str, source: &str, translated: &str, answer: &str) -> String {
    let mut out = template.to_string();
    out = out.replace("{source}", source);
    out = out.replace("{translated}", translated);
    out = out.replace("{answer}", answer);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_translate_prompt_has_target_lang() {
        let client = GrokClient::new("key".to_string());
        let _ = &client; // 構築できること
        let prompt = translate_prompt("英語");
        assert_eq!(prompt, "入力を英語に翻訳し、訳文だけを出力する。");
        // few-shot の「翻訳結果:」は訳文に混ざるので入れない
        assert!(!prompt.contains("翻訳結果"));
    }

    #[test]
    fn test_translate_request_model_and_effort() {
        let cases = [
            (
                TranslateModel::Grok420,
                "grok-4.20-0309-non-reasoning",
                None,
            ),
            (TranslateModel::Grok47, "grok-4.7", Some("low")),
            (TranslateModel::Grok45, "grok-4.5", Some("low")),
            (TranslateModel::Grok43, "grok-4.3", Some("none")),
        ];
        for (model, id, effort) in cases {
            let request = translate_request(model, "prompt", "こんにちは");
            let json: serde_json::Value = serde_json::to_value(&request).unwrap();
            assert_eq!(json["model"], id);
            match effort {
                Some(e) => assert_eq!(json["reasoning_effort"], e),
                // reasoning なしのモデルにはキーごと送らない
                None => assert!(json.get("reasoning_effort").is_none()),
            }
            assert_eq!(json["messages"][0]["role"], "system");
            assert_eq!(json["messages"][0]["content"], "prompt");
            assert_eq!(json["messages"][1]["role"], "user");
            assert_eq!(json["messages"][1]["content"], "こんにちは");
        }
    }

    #[test]
    fn test_proofread_request_stays_on_grok_4_7_low() {
        let request = chat_request(MODEL, Some(REASONING_EFFORT), PROOFREAD_PROMPT, "えーと");
        let json: serde_json::Value = serde_json::to_value(&request).unwrap();
        assert_eq!(json["model"], "grok-4.7");
        assert_eq!(json["reasoning_effort"], "low");
    }

    #[test]
    fn test_question_prompt_has_style_and_search_instruction() {
        // 話し方の指針は残し、アニメキャラのペルソナは持ち込まない
        assert!(QUESTION_PROMPT.contains("話し方"));
        assert!(!QUESTION_PROMPT.contains("persona"));
        assert!(QUESTION_PROMPT.contains("VRChat"));
        assert!(QUESTION_PROMPT.contains("web_search"));
        // memory は移植しない
        assert!(!QUESTION_PROMPT.contains("conversation_history"));
    }

    #[test]
    fn test_proofread_prompt_forbids_translation() {
        assert!(PROOFREAD_PROMPT.contains("フィラー"));
        assert!(PROOFREAD_PROMPT.contains("翻訳しない"));
    }

    #[test]
    fn test_chat_request_serializes_model_and_effort() {
        let request = ChatRequest {
            model: MODEL,
            reasoning_effort: Some(REASONING_EFFORT),
            messages: vec![ChatMessage {
                role: "user",
                content: "hi",
            }],
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"reasoning_effort\":\"low\""));
        assert!(json.contains("\"model\":\"grok-4.7\""));
        assert!(!json.contains("search_parameters"));
        assert!(!json.contains("tools"));
    }

    #[test]
    fn test_responses_request_serializes_builtin_tools() {
        let request = ResponsesRequest {
            model: MODEL,
            reasoning: Reasoning {
                effort: REASONING_EFFORT,
            },
            input: vec![ChatMessage {
                role: "user",
                content: "hi",
            }],
            tools: vec![
                BuiltinTool {
                    tool_type: "web_search",
                },
                BuiltinTool {
                    tool_type: "x_search",
                },
            ],
            text: TextFormat {
                format: JsonSchemaFormat {
                    format_type: "json_schema",
                    name: "question_answer",
                    strict: true,
                    schema: question_schema(),
                },
            },
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"reasoning\":{\"effort\":\"low\"}"));
        assert!(json.contains("\"type\":\"web_search\""));
        assert!(json.contains("\"type\":\"x_search\""));
        assert!(!json.contains("search_parameters"));
        // 構造化出力: answer を最後に持つスキーマを送る
        assert!(json.contains("\"type\":\"json_schema\""));
        assert!(json.contains("\"name\":\"question_answer\""));
        let answer_pos = json.find("\"answer\"").unwrap();
        let citation_pos = json.find("\"citation\"").unwrap();
        assert!(citation_pos < answer_pos);
    }

    #[test]
    fn test_render_template_replaces_known_placeholders() {
        assert_eq!(
            render_template("{source}/{translated}", "こんにちは", "Hello", ""),
            "こんにちは/Hello"
        );
        assert_eq!(
            render_template("> {source}\nAI> {answer}", "何これ", "", "それは猫"),
            "> 何これ\nAI> それは猫"
        );
    }

    #[test]
    fn test_render_template_keeps_unknown_placeholders() {
        assert_eq!(
            render_template("{unknown} {source}", "a", "b", "c"),
            "{unknown} a"
        );
    }
}
