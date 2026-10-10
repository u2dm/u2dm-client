use super::catalog::{Flag, Scenarios};
use super::media;
use std::env;
use std::sync::OnceLock;

use crate::domain::message::{MessageBody, RichText, SendState, TimelineMessage};

const ENV_VAR: &str = "U2DM_DEMO_RICHTEXT";

pub const CATALOG: Scenarios = Scenarios {
    env: ENV_VAR,
    summary: "injects formatted and adversarial message bodies over the fixture",
    combinable: true,
    flags: &[
        Flag {
            value: "adversarial",
            effect: "hostile HTML, so the sanitiser is exercised",
            note: "",
        },
        Flag {
            value: "hard",
            effect: "structurally hard but legitimate formatting",
            note: "",
        },
        Flag {
            value: "links",
            effect: "messages carrying rich links",
            note: "",
        },
        Flag {
            value: "bare",
            effect: "bare URLs, which are linkified rather than parsed",
            note: "",
        },
        Flag {
            value: "long",
            effect: "oversized bodies",
            note: "",
        },
        Flag {
            value: "emoji",
            effect: "custom emoji alone and among words, one animated and one missing",
            note: "",
        },
        Flag {
            value: "all",
            effect: "adversarial, hard, links, bare, long and emoji",
            note: "",
        },
    ],
    notes: &[],
};

const ADVERSARIAL: &[(&str, &str)] = &[
    (
        "*not italic* _not italic_ `not code` **not bold**",
        "*not italic* _not italic_ `not code` **not bold**",
    ),
    (
        "[not a link](https://evil.invalid) &lt;u&gt;not underlined&lt;/u&gt;",
        "[not a link](https://evil.invalid) <u>not underlined</u>",
    ),
    (
        "# not a heading\n&gt; not a quote\n    not a code block\n--- not a rule",
        "# not a heading<br>> not a quote<br>    not a code block<br>--- not a rule",
    ),
    (
        "backslash \\ and pipe | and tilde ~~~ and bang !",
        "backslash \\ and pipe | and tilde ~~~ and bang !",
    ),
];

const HARD: &[(&str, &str)] = &[
    (
        "A heading, a quote and a code block walk into a room.",
        "<h2>A heading</h2><blockquote>A quote, <em>emphasised</em>.</blockquote>\
         <pre><code>fn main() {\n    println!(\"hi\");\n}</code></pre>",
    ),
    (
        "Nested lists and a table.",
        "<ul><li>first<ul><li>nested</li><li>also nested</li></ul></li><li>second</li></ul>\
         <ol><li>one</li><li>two</li></ol>\
         <table><tr><th>name</th><th>value</th></tr><tr><td>alpha</td><td>1</td></tr></table>",
    ),
    (
        "A custom emote and a rule.",
        "before <img src=\"mxc://demo.local/emote\" alt=\":party:\" /> after<hr/>done",
    ),
    (
        "Spoilers and colours.",
        "<span data-mx-spoiler>the butler did it</span> and \
         <span data-mx-color=\"#e05252\">red</span> and <font color=\"nonsense\">ignored</font>",
    ),
    (
        "Unclosed markup that must not blank the message.",
        "<strong>bold without an end <em>and emphasis",
    ),
    (
        "A reply quoting two lines, as Element sends it.",
        "<blockquote>\n<p>They said this<br />\nover two lines</p>\n</blockquote>\n\
         <p>and this is my answer</p>\n",
    ),
    (
        "Loose lists, as Element sends them.",
        "<ul>\n<li>\n<p>first point</p>\n</li>\n<li>\n<p>second point</p>\n\
         <ol>\n<li>a detail</li>\n<li>another</li>\n</ol>\n</li>\n</ul>\n\
         <ol start=\"3\">\n<li>\n<p>third</p>\n</li>\n<li>\n<p><strong>fourth</strong></p>\n</li>\n</ol>\n",
    ),
];

const LINKS: &[(&str, &str)] = &[
    (
        "Docs, mail and a matrix URI.",
        "See <a href=\"https://spec.matrix.org/latest/client-server-api/\">the spec</a>, \
         mail <a href=\"mailto:nobody@example.invalid\">nobody</a>, or open \
         <a href=\"matrix:r/demo:demo.local\">a matrix URI</a> that must be refused.",
    ),
    (
        "A link with styling inside it.",
        "<a href=\"https://example.invalid/very/long/path/that/keeps/going/and/going/for/a/while\">\
         <strong>bold</strong> link text</a>",
    ),
    (
        "a refused scheme",
        "<a href=\"matrix:r/demo:demo.local\">a refused scheme</a>",
    ),
    (
        "Sarah, Alex and Nadia, tagged three ways.",
        "<a href=\"https://matrix.to/#/@sarah:matrix.org\">Sarah</a>, \
         <a href=\"https://matrix.to/#/%40alex%3Amatrix.org?via=matrix.org\">Alex</a> and \
         <a href=\"matrix:u/nadia:matrix.org?action=chat\">Nadia</a>, tagged three ways.",
    ),
    (
        "Alex, in bold and in italics.",
        "<strong><a href=\"https://matrix.to/#/@alex:matrix.org\">Alex</a></strong>, in bold and \
         <em>in <a href=\"https://matrix.to/#/@alex:matrix.org\">Alex</a> italics</em>.",
    ),
];

const EMOJI: &[(&str, &str)] = &[
    (
        ":small:",
        "<img data-mx-emoticon=\"\" src=\"mxc://demo.local/demo-sticker-small\" alt=\":small:\" \
         title=\":small:\" height=\"32\">",
    ),
    (
        ":ship: :anim: :tall:",
        "<img data-mx-emoticon src=\"mxc://demo.local/demo-sticker-ship\" alt=\":ship:\"> \
         <img data-mx-emoticon src=\"mxc://demo.local/demo-sticker-anim\" alt=\":anim:\"> \
         <img data-mx-emoticon src=\"mxc://demo.local/demo-sticker-tall\" alt=\":tall:\">",
    ),
    (
        "nice work :anim:",
        "nice work <img data-mx-emoticon src=\"mxc://demo.local/demo-sticker-anim\" \
         alt=\":anim:\">",
    ),
    (
        ":ship: ships today, :small: so good, with the release notes and cargo run",
        "<img data-mx-emoticon src=\"mxc://demo.local/demo-sticker-ship\" alt=\":ship:\"> ships \
         <strong>today</strong>, <em><img data-mx-emoticon \
         src=\"mxc://demo.local/demo-sticker-small\" alt=\":small:\"> so good</em>, with \
         <a href=\"https://example.invalid/notes\">the release notes</a> and \
         <code>cargo run</code>",
    ),
    (
        "A paragraph long enough to wrap across several lines of the bubble, so the emote \
         :anim: in its middle must flow with the words around it, and one more at the very end \
         :small:",
        "A paragraph long enough to wrap across several lines of the bubble, so the emote \
         <img data-mx-emoticon src=\"mxc://demo.local/demo-sticker-anim\" alt=\":anim:\"> in its \
         middle must flow with the words around it, and one more at the very end \
         <img data-mx-emoticon src=\"mxc://demo.local/demo-sticker-small\" alt=\":small:\">",
    ),
    (
        "A quote, lists and two lines.",
        "<blockquote>quoted <img data-mx-emoticon src=\"mxc://demo.local/demo-sticker-small\" \
         alt=\":small:\"></blockquote><ul><li>first <img data-mx-emoticon \
         src=\"mxc://demo.local/demo-sticker-ship\" alt=\":ship:\"></li><li><img data-mx-emoticon \
         src=\"mxc://demo.local/demo-sticker-anim\" alt=\":anim:\"> second<ul><li>nested \
         <img data-mx-emoticon src=\"mxc://demo.local/demo-sticker-tall\" alt=\":tall:\"></li></ul>\
         </li></ul><ol start=\"4\"><li>fourth <img data-mx-emoticon \
         src=\"mxc://demo.local/demo-sticker-small\" alt=\":small:\"></li></ol>one line<br>\
         <img data-mx-emoticon src=\"mxc://demo.local/demo-sticker-ship\" alt=\":ship:\"> and the \
         next",
    ),
    (
        "missing :missing: and remote :remote:",
        "missing <img data-mx-emoticon src=\"mxc://demo.local/demo-sticker-missing\" \
         alt=\":missing:\"> and remote <img src=\"https://example.invalid/remote.png\" \
         alt=\":remote:\">",
    ),
];

const BARE: &[&str] = &[
    "no formatting at all, just https://spec.matrix.org/latest/ in the middle",
    "trailing punctuation: see https://example.invalid/docs. Then (https://example.invalid/more), \
     and <https://example.invalid/angled>",
    "www.example.invalid/path and nobody@example.invalid and mailto:someone@example.invalid",
    "user tags: @sarah:matrix.org, (@alex:matrix.org) and @nadia:matrix.org: hi",
    "not links: notes.rs, 3:2, @12:30, @room, @everyone: and nothttps://example.invalid",
];

#[derive(Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct Scenario {
    pub adversarial: bool,
    pub hard: bool,
    pub links: bool,
    pub bare: bool,
    pub oversized: bool,
    pub emoji: bool,
}

impl Scenario {
    fn any(self) -> bool {
        self.adversarial || self.hard || self.links || self.bare || self.oversized || self.emoji
    }
}

pub fn scenario() -> Scenario {
    static SCENARIO: OnceLock<Scenario> = OnceLock::new();
    *SCENARIO.get_or_init(from_env)
}

fn from_env() -> Scenario {
    let Ok(raw) = env::var(ENV_VAR) else {
        return Scenario::default();
    };
    let mut scenario = Scenario::default();
    for flag in raw.split(',').map(str::trim) {
        apply(&mut scenario, flag);
    }
    tracing::info!(
        adversarial = scenario.adversarial,
        hard = scenario.hard,
        links = scenario.links,
        bare = scenario.bare,
        oversized = scenario.oversized,
        emoji = scenario.emoji,
        "demo mode: injecting formatted messages"
    );
    scenario
}

fn apply(scenario: &mut Scenario, flag: &str) {
    match flag {
        "adversarial" => scenario.adversarial = true,
        "hard" => scenario.hard = true,
        "links" => scenario.links = true,
        "bare" => scenario.bare = true,
        "long" => scenario.oversized = true,
        "emoji" => scenario.emoji = true,
        "all" => {
            scenario.adversarial = true;
            scenario.hard = true;
            scenario.links = true;
            scenario.bare = true;
            scenario.oversized = true;
            scenario.emoji = true;
        }
        other => tracing::warn!("unknown {ENV_VAR} flag: {other}"),
    }
}

pub fn apply_scenario(messages: &mut Vec<TimelineMessage>) {
    let scenario = scenario();
    if !scenario.any() {
        return;
    }
    let Some(template) = messages.iter().rev().find(|m| !m.is_own).cloned() else {
        return;
    };

    let mut injected = Vec::new();
    if scenario.adversarial {
        extend(&mut injected, &template, "adversarial", ADVERSARIAL);
    }
    if scenario.hard {
        extend(&mut injected, &template, "hard", HARD);
    }
    if scenario.links {
        extend(&mut injected, &template, "links", LINKS);
    }
    if scenario.bare {
        extend_plain(&mut injected, &template, "bare", BARE);
    }
    if scenario.oversized {
        let html = format!("<p>{}</p>", "a very long formatted run. ".repeat(4000));
        extend(
            &mut injected,
            &template,
            "long",
            &[("An oversized formatted body.", html.as_str())],
        );
    }
    if scenario.emoji {
        extend(&mut injected, &template, "emoji", EMOJI);
    }
    messages.extend(injected);
}

fn extend(
    out: &mut Vec<TimelineMessage>,
    template: &TimelineMessage,
    group: &str,
    cases: &[(&str, &str)],
) {
    for (index, (plain, html)) in cases.iter().enumerate() {
        let body = RichText::formatted((*plain).to_owned(), (*html).to_owned());
        out.push(message(template, group, index, body));
    }
}

fn extend_plain(
    out: &mut Vec<TimelineMessage>,
    template: &TimelineMessage,
    group: &str,
    cases: &[&str],
) {
    for (index, plain) in cases.iter().enumerate() {
        out.push(message(
            template,
            group,
            index,
            RichText::plain((*plain).to_owned()),
        ));
    }
}

fn message(
    template: &TimelineMessage,
    group: &str,
    index: usize,
    body: RichText,
) -> TimelineMessage {
    let id = format!("demo-richtext-{group}-{index}");
    let body = MessageBody::Text(body);
    TimelineMessage {
        unique_id: id.clone(),
        event_id: Some(id),
        local_id: None,
        body_emoji: media::emoji_states(&body),
        body,
        mentions_room: false,
        reply: None,
        edited: false,
        is_first_unread: false,
        send_state: SendState::default(),
        reactions: Vec::new(),
        ..template.clone()
    }
}
