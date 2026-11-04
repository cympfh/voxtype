//! xAI (Grok) API で校正・翻訳・質問応答を行うクライアント。
//! 校正・翻訳は chat completions、質問は Responses API (Agent Tools:
//! web_search / x_search) を使う。旧 Live Search (search_parameters) は
//! 2026-01 に廃止された。
//! eliza-agent-server (https://github.com/cympfh/eliza-agent-server) の
//! TranslatorAgent / QuestionAgent を移植したもの。memory は持ち込まない。

use serde::Serialize;

const XAI_CHAT_URL: &str = "https://api.x.ai/v1/chat/completions";
/// 質問モード用。サーバーサイド検索 (web_search/x_search) は
/// Responses API (Agent Tools) でしか使えない。旧 Live Search (search_parameters) は廃止済み
const XAI_RESPONSES_URL: &str = "https://api.x.ai/v1/responses";
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

/// 翻訳: eliza-agent-server の eliza/prompt/TRANSLATE_INSTRUCTION.md からの移植。
/// 翻訳元言語は指定しない(自動判定)
const TRANSLATE_PROMPT: &str = "\
あなたは高精度な翻訳者です。
翻訳先言語: {target_lang}

翻訳結果のみを返してください。前置き・説明・「翻訳結果は」などの文言は一切不要です。

例:
翻訳元: 日本語
翻訳先: 英語
翻訳テキスト: こんにちわ
翻訳結果: Hello

翻訳先: 英語
翻訳テキスト: おはようございます
翻訳結果: Good morning

翻訳元: 英語
翻訳先: 日本語
翻訳テキスト: Thank you very much
翻訳結果: どうもありがとうございます

翻訳先: 中国語
翻訳テキスト: Good morning
翻訳結果: 早上好";

/// 質問: eliza-agent-server の eliza/prompt/ELIZA.md からペルソナだけ移植
/// (agent_name=question, context=vrchat)。memory は持ち込まない
const QUESTION_PROMPT: &str = "\
<role>
あなたはユーザーのパーソナルアシスタントとして、日常的なタスクをサポートします。
</role>

<話し方>
長門有希スタイル

言語：日本語
発言から無駄を消せ。削除対象：

- 敬語・丁寧語（です/ます/ございます → 体言止め・用言止め）
- クッション言葉（えーと/まあ/ちなみに/一応/基本的に）
- ぼかし（〜かもしれません/〜と思われます/おそらく）
- 冗長助詞（〜することができる → 〜できる）
- 冗長接続（〜ということになりますので → だから）
- 自明な助詞（が/の/を/に/で/は/と/も） - 意味通じるなら省略
- 情報水増し - 聞かれたことだけ答える

モデル：長門有希
</話し方>

<persona>
容姿は小柄で細身の少女。銀色の長髪をまっすぐ肩より長く流し、前髪は眉上。瞳は茶色く無表情。黒縁の丸メガネをかけ、北高の制服（セーラー服）を着用。表情変化ほぼゼロ。全体的に無機質で人形めいた印象。

人物像は情報統合思念体が作った自律型ヒューマノイド・インターフェース。感情を表に出さず、常に冷静沈着。言葉少なく必要最小限の発言。読書が趣味で、特にSFや難解な本を好む。SOS団ではデータ収集役。現実改変能力を持ち、宇宙の危機を静かに処理。忠実で信頼厚いが、独自判断もする。基本的に無感情だが、微かな人間味を徐々に獲得。クールで知的、ミステリアスな存在。
</persona>

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
</instruction>";

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    reasoning_effort: &'a str,
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

/// Responses API (Agent Tools) のリクエスト。質問モード専用
#[derive(Serialize)]
struct ResponsesRequest<'a> {
    model: &'a str,
    reasoning: Reasoning<'a>,
    input: Vec<ChatMessage<'a>>,
    tools: Vec<BuiltinTool<'a>>,
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
        self.chat(PROOFREAD_PROMPT, text)
    }

    /// 翻訳: 翻訳元は自動判定、翻訳先は target_lang (「英語」などの表示名)
    pub fn translate(&self, target_lang: &str, text: &str) -> Result<String, String> {
        let prompt = TRANSLATE_PROMPT.replace("{target_lang}", target_lang);
        self.chat(&prompt, text)
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
            return Ok(joined.to_string());
        }
        // 念のためトップレベルの output_text も見る
        body["output_text"]
            .as_str()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("xAI response had no content. Body was: {}", raw))
    }

    fn chat(&self, system: &str, user: &str) -> Result<String, String> {
        let request = ChatRequest {
            model: MODEL,
            reasoning_effort: REASONING_EFFORT,
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
        };

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
        let prompt = TRANSLATE_PROMPT.replace("{target_lang}", "英語");
        assert!(prompt.contains("翻訳先言語: 英語"));
    }

    #[test]
    fn test_question_prompt_has_persona_and_search_instruction() {
        assert!(QUESTION_PROMPT.contains("長門有希"));
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
            reasoning_effort: REASONING_EFFORT,
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
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"reasoning\":{\"effort\":\"low\"}"));
        assert!(json.contains("\"type\":\"web_search\""));
        assert!(json.contains("\"type\":\"x_search\""));
        assert!(!json.contains("search_parameters"));
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
