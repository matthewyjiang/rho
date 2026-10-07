# Decisions

The `decision` module asks typed questions about a state and returns one typed answer per question. A decision model answers them over the System One API (`POST /v1/systemone`): TypeSafe's Jev, Cloudflare's Clef, and the decision models Ollama serves locally; or over OpenAI's Decisions API (`POST /v1/decisions`): `gpt-6-luna`. A text model answers the same questions through `decision::text::TextDecisionModel`, so a feature written against `DecisionModel` runs on either.

## Questions

A `DecisionRequest` has shared instructions, a state, and up to 64 questions. The state is evidence only: put every rule the answers must follow in the instructions or the questions.

| Kind | Constructor | Answer |
| --- | --- | --- |
| Yes or no | `Question::noul(id, instructions, criteria)`. `criteria` optionally describes what yes and no mean. | `NoulAnswer`: `value()` is yes or no, and `probability()` is the probability of yes. |
| Pick one | `Question::choice(id, instructions, options)`: 2 to 26 `ChoiceOption`s, each an ID and a description. | `ChoiceAnswer`: `option()` indexes the options, and `probabilities()` gives each option's probability, in option order. |
| Rating | `Question::score(id, instructions, levels)`: 2 to 10 level descriptions, lowest first. | `ScoreAnswer`: `value()` is the zero-based level, and `probabilities()` gives each level's probability. |

`Question::check` and `DecisionRequest::check` apply the smallest limits of the published servers, so a request that passes runs on any of them:

| Limit | Value | Source |
| --- | --- | --- |
| Questions per request | 1 to 64 | Workers AI and Ollama accept at most 64 |
| Choice options | 2 to 26 | Ollama accepts 2 to 26; TypeSafe accepts 255 |
| Score levels | 2 to 10 | TypeSafe accepts 2 to 10; Ollama accepts 26 |
| Question and option IDs | 1 to 100 ASCII letters, digits, `_`, `.`, or `-` | Workers AI |

Both checks are `const fn`, so a fixed question can fail the build:

```rust
use rho_sdk::decision::{ChoiceOption, Question};

const TEAM: Question<'static> = Question::choice(
    "team",
    "Which team should handle this request?",
    &[
        ChoiceOption::new("billing", "Payments, invoices, and refunds"),
        ChoiceOption::new("technical", "Outages, errors, and configuration"),
    ],
);
const _: () = assert!(TEAM.check().is_ok());
```

## Answers and probabilities

`DecisionModel::decide` returns one `Answer` per question, in question order and of the question's kind, or a `DecisionError`. It never returns a default answer, so a caller can fail closed.

A decision model reports a probability for every possible answer. A noul answer is yes when its probability is at least 0.5. A score's value is the probability-weighted level, so it can fall between levels.

A text model reports no probabilities, and its `probability()` and `probabilities()` return `None` rather than a stand-in certainty. A caller that sets a probability threshold decides what a text model's answer means.

Answer constructors reject probabilities that are not a distribution: each must be within 0 to 1, and together they must total 1, give or take 0.005 per probability for servers that round to 2 decimals (TypeSafe does). A choice that reports probabilities must also be a most likely option, ties included. A broken response therefore cannot become a stronger answer. `DecisionRequest::check_answers` checks that answers fit the request's questions: their count, kind, option or level, and one probability per option or level. Call it before acting on answers from a model you did not write.

## Text models

`TextDecisionModel::new(provider, style)` asks one provider turn per request, with no tools:

- the shared instructions become the system prompt, after a short protocol preamble (`decision::text::system_prompt`);
- the state is the first user block;
- the questions are the second user block (`decision::text::questions_block`).

Keep the instructions and state byte-identical across requests about one state, and only the questions block misses the prompt cache.

Each question becomes a pick-one over answer IDs: option IDs for a choice, `true` or `false` for a noul, and the zero-based level for a score. `AnswerStyle::Direct` reads the whole response as a JSON object mapping question IDs to answer IDs. `AnswerStyle::Reasoned` lets the model reason first and reads only the final line. Anything else is an error that never repeats the response. Hosts that run text models their own way can reuse `input` and `parse_answers`.

## Decision-model clients

The core crate has no network client. `rho-providers` has `system_one::SystemOneModel`, a System One client that takes an API base such as `http://localhost:11434/v1` or `https://api.typesafe.ai/v1`, a model name, and an optional API key sent as a bearer token. `SystemOneLimits` makes a request over a server's body limit or a model's state budget fail before it is sent, naming the limit and the asked size; `SystemOneLimits::OLLAMA` holds Ollama's.

`openai_decisions::OpenAiDecisionsModel` is an OpenAI Decisions API client with the same arguments, for an API base such as `https://api.openai.com/v1` (`OPENAI_API_BASE`). A noul question is sent as a `predicate`, with its criteria appended to its instructions; a score level is sent with its zero-based index as its label. Its state budget is `gpt-6-luna`'s 922,000 input tokens with headroom.

`DecisionModel::state_budget` reports the largest state a model accepts, in `model::context::estimate_text_tokens`. A caller with a longer state shortens it first.

## Implementor contract

A `DecisionModel` implementation must:

1. answer every question, in order, with an answer of its kind, or return an error (`DecisionRequest::check_answers` holds the shape to check against)
2. build answers with the `from_probabilities` constructors only when the model reported probabilities
3. observe the cancellation token
4. keep response text and credentials out of error messages; `DecisionError::InvalidResponse` names the problem, not the response
