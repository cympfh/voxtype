//! xAI (Grok) の chat completions API で校正・翻訳・質問応答を行うクライアント。
//! eliza-agent-server (https://github.com/cympfh/eliza-agent-server) の
//! TranslatorAgent / QuestionAgent を移植したもの。memory は持ち込まない。

use serde::Serialize;

const XAI_CHAT_URL: &str = "https://api.x.ai/v1/chat/completions";
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
struct SearchSource<'a> {
    #[serde(rename = "type")]
    source_type: &'a str,
}

#[derive(Serialize)]
struct SearchParameters<'a> {
    mode: &'a str,
    sources: Vec<SearchSource<'a>>,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    reasoning_effort: &'a str,
    messages: Vec<ChatMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_parameters: Option<SearchParameters<'a>>,
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
        self.chat(PROOFREAD_PROMPT, text, false)
    }

    /// 翻訳: 翻訳元は自動判定、翻訳先は target_lang (「英語」などの表示名)
    pub fn translate(&self, target_lang: &str, text: &str) -> Result<String, String> {
        let prompt = TRANSLATE_PROMPT.replace("{target_lang}", target_lang);
        self.chat(&prompt, text, false)
    }

    /// 質問: web/X の検索(Live Search)を使って答える
    pub fn question(&self, text: &str) -> Result<String, String> {
        self.chat(QUESTION_PROMPT, text, true)
    }

    fn chat(&self, system: &str, user: &str, live_search: bool) -> Result<String, String> {
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
            search_parameters: live_search.then(|| SearchParameters {
                mode: "auto",
                sources: vec![
                    SearchSource { source_type: "web" },
                    SearchSource { source_type: "x" },
                ],
            }),
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
    fn test_request_serializes_search_parameters_only_when_live_search() {
        let with_search = ChatRequest {
            model: MODEL,
            reasoning_effort: REASONING_EFFORT,
            messages: vec![ChatMessage {
                role: "user",
                content: "hi",
            }],
            search_parameters: Some(SearchParameters {
                mode: "auto",
                sources: vec![SearchSource { source_type: "web" }],
            }),
        };
        let json = serde_json::to_string(&with_search).unwrap();
        assert!(json.contains("\"search_parameters\""));
        assert!(json.contains("\"mode\":\"auto\""));

        let without = ChatRequest {
            model: MODEL,
            reasoning_effort: REASONING_EFFORT,
            messages: vec![],
            search_parameters: None,
        };
        let json = serde_json::to_string(&without).unwrap();
        assert!(!json.contains("search_parameters"));
        assert!(json.contains("\"reasoning_effort\":\"low\""));
        assert!(json.contains("\"model\":\"grok-4.7\""));
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
