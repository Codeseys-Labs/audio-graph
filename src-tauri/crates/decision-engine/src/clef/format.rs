//! Port of `encode_record` and `systemone_answer` from the checkpoint's `joint_schema_model.py`.
//!
//! The prompt is assembled from separately tokenized segments (the reference tokenizes each piece on its
//! own and concatenates the ids), so token boundaries never cross a segment. The committed fixture pins
//! the ids and spans (`fixtures/clef-flash.json`).

use crate::pyjson::{self, Style};
use audio_graph_decision_contract::{Answer, DecisionError, DecisionState, OrderedMap, Question};
use serde_json::{Map, Value};
use tokenizers::Tokenizer;

/// Total prompt budget of the reference and of vLLM's ClefFlashStrategy; the state is truncated to fit.
pub const MAX_LENGTH: usize = 16_384;

const SYSTEM_PROMPT: &str = "Read the complete state and schema. Decide every field jointly. Each answer \
must be exactly one of that field's allowed options.";
const NOUL_TRUE: &str = "The proposition is true or the answer is yes.";
const NOUL_FALSE: &str = "The proposition is false or the answer is no.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Noul,
    Choice,
    Score,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Noul => "noul",
            Self::Choice => "choice",
            Self::Score => "score",
        }
    }

    /// Row of the head's `type_embedding` (`QUESTION_TYPES` in the reference).
    pub fn type_id(self) -> u32 {
        match self {
            Self::Noul => 0,
            Self::Choice => 1,
            Self::Score => 2,
        }
    }
}

/// One question's place in the prompt. Spans are half-open token ranges of the full input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedQuestion {
    pub id: String,
    pub kind: Kind,
    pub question_span: (usize, usize),
    pub option_spans: Vec<(usize, usize)>,
    /// Option ids in prompt order: `true, false` for noul, keys sorted for choice, level indices for score.
    pub option_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub input_ids: Vec<u32>,
    pub questions: Vec<EncodedQuestion>,
    /// State tokens dropped to fit `MAX_LENGTH`.
    pub truncated_state_tokens: usize,
}

/// `render`: strings verbatim, any other JSON value compact with sorted keys.
pub fn render_state(state: &DecisionState) -> String {
    match state {
        DecisionState::Text(text) => text.clone(),
        DecisionState::Json(Value::String(text)) => text.clone(),
        DecisionState::Json(value) => pyjson::dumps(value, Style::CompactSorted),
    }
}

/// (option id, description) pairs of one question, in prompt order.
type Options = Vec<(String, Option<String>)>;

/// `question_options`: the question kind, its instructions and its options in prompt order.
fn options(question: &Question) -> Result<(Kind, &str, Options), DecisionError> {
    Ok(match question {
        Question::Noul {
            instructions,
            criteria,
        } => {
            let (t, f) = criteria.as_ref().map_or((None, None), |c| {
                (c.true_desc.clone(), c.false_desc.clone())
            });
            (
                Kind::Noul,
                instructions,
                vec![
                    ("true".into(), Some(t.unwrap_or_else(|| NOUL_TRUE.into()))),
                    ("false".into(), Some(f.unwrap_or_else(|| NOUL_FALSE.into()))),
                ],
            )
        }
        Question::Choice {
            instructions,
            criteria,
        } => {
            if criteria.is_empty() {
                return Err(DecisionError::UnsupportedQuestion);
            }
            let mut options: Vec<(String, Option<String>)> = criteria
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            // Python sorts (key, description) tuples; keys are unique, so this is a key sort by code point.
            options.sort_by(|a, b| a.0.cmp(&b.0));
            (Kind::Choice, instructions, options)
        }
        Question::Score {
            instructions,
            criteria,
        } => {
            if criteria.is_empty() {
                return Err(DecisionError::UnsupportedQuestion);
            }
            let options = criteria
                .iter()
                .enumerate()
                .map(|(i, level)| (i.to_string(), Some(level.clone())))
                .collect();
            (Kind::Score, instructions, options)
        }
    })
}

fn tokens(tokenizer: &Tokenizer, text: &str) -> Result<Vec<u32>, DecisionError> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    tokenizer
        .encode(text, false)
        .map(|e| e.get_ids().to_vec())
        .map_err(|_| DecisionError::Internal)
}

/// `encode_record`: the full prompt token ids and every question's spans.
pub fn encode(
    tokenizer: &Tokenizer,
    state: &DecisionState,
    questions: &OrderedMap<String, Question>,
) -> Result<Encoded, DecisionError> {
    if questions.is_empty() {
        return Err(DecisionError::UnsupportedQuestion);
    }
    let mut schema = tokens(tokenizer, "\n\nSCHEMA FIELDS:\n")?;
    let mut encoded_questions = Vec::with_capacity(questions.len());
    for (index, (id, question)) in questions.iter().enumerate() {
        if id.is_empty() {
            return Err(DecisionError::UnsupportedQuestion);
        }
        let (kind, instructions, options) = options(question)?;
        schema.extend(tokens(
            tokenizer,
            &format!(
                "\nFIELD {}\nID: {id}\nTYPE: {}\nINSTRUCTION: ",
                index + 1,
                kind.name()
            ),
        )?);
        let question_start = schema.len();
        let instructions = if instructions.is_empty() {
            id.as_str()
        } else {
            instructions
        };
        schema.extend(tokens(tokenizer, instructions)?);
        let question_span = (question_start, schema.len());
        schema.extend(tokens(tokenizer, "\nALLOWED OPTIONS:\n")?);
        let mut option_spans = Vec::with_capacity(options.len());
        let mut option_ids = Vec::with_capacity(options.len());
        for (option_index, (option_id, description)) in options.into_iter().enumerate() {
            schema.extend(tokens(
                tokenizer,
                &format!("OPTION {}: ", option_index + 1),
            )?);
            let start = schema.len();
            let mut semantics = Map::new();
            semantics.insert("option_id".into(), Value::String(option_id.clone()));
            if let Some(description) = description {
                semantics.insert("description".into(), Value::String(description));
            }
            schema.extend(tokens(
                tokenizer,
                &pyjson::dumps(&Value::Object(semantics), Style::CompactSorted),
            )?);
            option_spans.push((start, schema.len()));
            option_ids.push(option_id);
            schema.extend(tokens(tokenizer, "\n")?);
        }
        schema.extend(tokens(tokenizer, "END FIELD\n")?);
        encoded_questions.push(EncodedQuestion {
            id: id.clone(),
            kind,
            question_span,
            option_spans,
            option_ids,
        });
    }
    let prefix = tokens(
        tokenizer,
        &format!("<|im_start|>system\n{SYSTEM_PROMPT}<|im_end|>\n<|im_start|>user\nSTATE:\n"),
    )?;
    let suffix = tokens(
        tokenizer,
        "\n<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\nJOINT SCHEMA DECISIONS:",
    )?;
    let mut state_ids = tokens(tokenizer, &render_state(state))?;
    let fixed = prefix.len() + schema.len() + suffix.len();
    if fixed > MAX_LENGTH {
        // The schema alone does not fit: there is no state left to drop.
        return Err(DecisionError::CardinalityLimit {
            question: String::new(),
            limit: MAX_LENGTH,
        });
    }
    let room = MAX_LENGTH - fixed;
    let truncated_state_tokens = state_ids.len().saturating_sub(room);
    state_ids.truncate(room);
    let offset = prefix.len() + state_ids.len();
    for question in &mut encoded_questions {
        question.question_span = (
            question.question_span.0 + offset,
            question.question_span.1 + offset,
        );
        for span in &mut question.option_spans {
            *span = (span.0 + offset, span.1 + offset);
        }
    }
    let mut input_ids = prefix;
    input_ids.extend(state_ids);
    input_ids.extend(schema);
    input_ids.extend(suffix);
    Ok(Encoded {
        input_ids,
        questions: encoded_questions,
        truncated_state_tokens,
    })
}

/// The contract answer from one question's option probabilities (prompt order). Values follow
/// `systemone_answer` without its 4-decimal rounding; `confidence` is the native-engine meaning, the
/// probability of the reported answer.
pub fn answer(
    question: &Question,
    encoded: &EncodedQuestion,
    probabilities: &[f32],
) -> Result<Answer, DecisionError> {
    if probabilities.len() != encoded.option_ids.len()
        || probabilities.iter().any(|p| !p.is_finite())
    {
        return Err(DecisionError::Internal);
    }
    let by_id = |id: &str| -> Result<f64, DecisionError> {
        encoded
            .option_ids
            .iter()
            .position(|o| o == id)
            .map(|i| f64::from(probabilities[i]))
            .ok_or(DecisionError::Internal)
    };
    Ok(match question {
        Question::Noul { .. } => {
            let (t, f) = (by_id("true")?, by_id("false")?);
            Answer::Noul {
                noul: t,
                probabilities: Some(
                    [("false".to_owned(), f), ("true".to_owned(), t)]
                        .into_iter()
                        .collect(),
                ),
                confidence: Some(t.max(f)),
            }
        }
        Question::Choice { criteria, .. } => {
            let probabilities: OrderedMap<String, f64> = criteria
                .keys()
                .map(|k| Ok((k.clone(), by_id(k)?)))
                .collect::<Result<_, DecisionError>>()?;
            // `max(options, key=...)` over request order: the first maximum wins.
            let (choice, confidence) = probabilities
                .iter()
                .fold(None::<(&String, f64)>, |best, (k, p)| match best {
                    Some((_, b)) if *p <= b => best,
                    _ => Some((k, *p)),
                })
                .map(|(k, p)| (k.clone(), p))
                .ok_or(DecisionError::Internal)?;
            Answer::Choice {
                choice,
                probabilities,
                confidence,
            }
        }
        Question::Score { criteria, .. } => {
            let probabilities: Vec<f64> = (0..criteria.len())
                .map(|i| by_id(&i.to_string()))
                .collect::<Result<_, _>>()?;
            Answer::Score {
                score: probabilities
                    .iter()
                    .enumerate()
                    .map(|(i, p)| i as f64 * p)
                    .sum(),
                confidence: probabilities.iter().copied().fold(0.0, f64::max),
                probabilities,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio_graph_decision_contract::NoulCriteria;

    #[test]
    fn json_state_is_compact_with_sorted_keys_and_strings_are_verbatim() {
        let v: Value =
            serde_json::from_str(r#"{"b": [1, {"d": 2, "c": "é"}], "a": null}"#).unwrap();
        assert_eq!(
            render_state(&DecisionState::Json(v)),
            r#"{"a":null,"b":[1,{"c":"é","d":2}]}"#
        );
        assert_eq!(
            render_state(&DecisionState::Json(Value::String("x".into()))),
            "x"
        );
        assert_eq!(render_state(&DecisionState::Text(String::new())), "");
    }

    #[test]
    fn options_follow_the_reference_order_and_defaults() {
        let choice = Question::Choice {
            instructions: "pick".into(),
            criteria: [
                ("zeta".to_owned(), None),
                ("alpha".to_owned(), Some("first".to_owned())),
            ]
            .into_iter()
            .collect(),
        };
        let (_, _, opts) = options(&choice).unwrap();
        assert_eq!(
            opts,
            vec![
                ("alpha".into(), Some("first".into())),
                ("zeta".into(), None)
            ]
        );
        let noul = Question::Noul {
            instructions: "is it?".into(),
            criteria: Some(NoulCriteria {
                true_desc: Some("yes".into()),
                false_desc: None,
            }),
        };
        let (_, _, opts) = options(&noul).unwrap();
        assert_eq!(
            opts,
            vec![
                ("true".into(), Some("yes".into())),
                ("false".into(), Some(NOUL_FALSE.into()))
            ]
        );
    }

    /// Choice answers are keyed in request order while the prompt lists options sorted; the pick is the
    /// first maximum in request order and `confidence` is its probability.
    #[test]
    fn answers_map_prompt_order_back_to_request_order() {
        let question = Question::Choice {
            instructions: "pick".into(),
            criteria: [("zeta".to_owned(), None), ("alpha".to_owned(), None)]
                .into_iter()
                .collect(),
        };
        let encoded = EncodedQuestion {
            id: "q".into(),
            kind: Kind::Choice,
            question_span: (0, 1),
            option_spans: vec![(1, 2), (2, 3)],
            option_ids: vec!["alpha".into(), "zeta".into()],
        };
        let Answer::Choice {
            choice,
            probabilities,
            confidence,
        } = answer(&question, &encoded, &[0.25, 0.75]).unwrap()
        else {
            panic!("choice")
        };
        assert_eq!(choice, "zeta");
        assert_eq!(probabilities.keys().collect::<Vec<_>>(), ["zeta", "alpha"]);
        assert_eq!(confidence, 0.75);
        let noul = Question::Noul {
            instructions: "x".into(),
            criteria: None,
        };
        let encoded = EncodedQuestion {
            option_ids: vec!["true".into(), "false".into()],
            kind: Kind::Noul,
            ..encoded
        };
        let Answer::Noul {
            noul, confidence, ..
        } = answer(&noul, &encoded, &[0.25, 0.75]).unwrap()
        else {
            panic!("noul")
        };
        assert_eq!((noul, confidence), (0.25, Some(0.75)));
    }
}
