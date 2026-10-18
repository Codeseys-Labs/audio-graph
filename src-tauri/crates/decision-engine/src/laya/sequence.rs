use crate::{Answer, DecisionError, DecisionState, OrderedMap, Question};
use serde::Deserialize;
use std::borrow::Cow;
use tokenizers::Tokenizer;

#[derive(Debug, Deserialize)]
pub(super) struct Config {
    pub max_len: usize,
    pub head_max_len: usize,
    pub temperature: [f64; 3],
    #[serde(default)]
    pub temperature_by_options: OrderedMap<String, f64>,
}

impl Config {
    pub fn temperature(&self, kind: usize, count: usize) -> f64 {
        let bucket = match count {
            0..=2 => "2",
            3..=5 => "3-5",
            6..=10 => "6-10",
            _ => "11+",
        };
        let key = format!("{}:{bucket}", ["choice", "score", "noul"][kind]);
        let value = self
            .temperature_by_options
            .get(&key)
            .copied()
            .filter(|t| t.is_finite())
            .unwrap_or(self.temperature[kind]);
        if value.is_finite() {
            value.clamp(0.5, 5.0)
        } else {
            1.0
        }
    }
}

pub(super) struct SpecialIds {
    pub cls: i64,
    pub sep: i64,
    pub mask: i64,
    pub pad: i64,
    pub mask_text: String,
}

pub(super) struct Sequence {
    pub ids: Vec<i64>,
    pub markers: Vec<i64>,
    pub kind: usize,
}

pub(super) struct StateWindow<'a> {
    pub tokens: &'a [i64],
    pub keep_newest: bool,
}

pub(super) fn options(question: &Question) -> Vec<String> {
    match question {
        Question::Choice { criteria, .. } => criteria
            .iter()
            .map(
                |(key, description)| match description.as_deref().filter(|s| !s.is_empty()) {
                    Some(description) => format!("{key}: {description}"),
                    None => key.clone(),
                },
            )
            .collect(),
        Question::Score { criteria, .. } => criteria
            .iter()
            .enumerate()
            .map(|(i, value)| format!("level {i}: {value}"))
            .collect(),
        Question::Noul { criteria, .. } => {
            let no = criteria
                .as_ref()
                .and_then(|c| c.false_desc.as_deref())
                .filter(|s| !s.is_empty())
                .unwrap_or("no, the statement does not hold");
            let yes = criteria
                .as_ref()
                .and_then(|c| c.true_desc.as_deref())
                .filter(|s| !s.is_empty())
                .unwrap_or("yes, the statement holds");
            vec![format!("false: {no}"), format!("true: {yes}")]
        }
    }
}

// Match json.dumps(..., ensure_ascii=False), including separators and insertion order.
pub(super) fn state_text(state: &DecisionState) -> Cow<'_, str> {
    fn render(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::Array(values) => format!(
                "[{}]",
                values.iter().map(render).collect::<Vec<_>>().join(", ")
            ),
            serde_json::Value::Object(values) => format!(
                "{{{}}}",
                values
                    .iter()
                    .map(|(key, value)| format!(
                        "{}: {}",
                        serde_json::to_string(key).expect("string serialization"),
                        render(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            _ => value.to_string(),
        }
    }
    match state {
        DecisionState::Text(text) => Cow::Borrowed(text),
        DecisionState::Json(value) => Cow::Owned(render(value)),
    }
}

pub(super) fn encode(
    tok: &Tokenizer,
    special: &SpecialIds,
    text: &str,
) -> Result<Vec<i64>, DecisionError> {
    let scrubbed = if text.contains(&special.mask_text) {
        Cow::Owned(text.replace(&special.mask_text, " "))
    } else {
        Cow::Borrowed(text)
    };
    tok.encode(scrubbed.as_ref(), false)
        .map(|encoding| encoding.get_ids().iter().map(|id| i64::from(*id)).collect())
        .map_err(|_| DecisionError::Internal)
}

pub(super) fn build(
    tok: &Tokenizer,
    special: &SpecialIds,
    config: &Config,
    state: StateWindow<'_>,
    question_id: &str,
    question: &Question,
    allow_large: bool,
) -> Result<Sequence, DecisionError> {
    let StateWindow {
        tokens: state_ids,
        keep_newest: newest_first,
    } = state;
    let (kind, instructions) = match question {
        Question::Choice { instructions, .. } => (0, instructions),
        Question::Score { instructions, .. } => (1, instructions),
        Question::Noul { instructions, .. } => (2, instructions),
    };
    let rendered = options(question);
    if rendered.is_empty() {
        return Err(DecisionError::UnsupportedQuestion);
    }
    if kind == 0 && rendered.len() > 20 && !allow_large {
        return Err(DecisionError::CardinalityLimit {
            question: question_id.into(),
            limit: 20,
        });
    }
    let encode = |text: &str| encode(tok, special, text);
    let mut head = encode(&format!(
        "{} question: {instructions}",
        ["choice", "score", "noul"][kind]
    ))?;
    let mut options = Vec::with_capacity(rendered.len());
    for option in &rendered {
        let mut ids = vec![special.mask];
        ids.extend(encode(&format!(" {option}"))?.into_iter().take(48));
        options.push(ids);
    }
    let head_max = config.head_max_len.min(192);
    let mut option_tokens: usize = options.iter().map(Vec::len).sum();
    if option_tokens.saturating_add(16) > head_max {
        let per = (head_max.saturating_sub(16) / options.len()).max(4);
        for option in &mut options {
            option.truncate(per);
        }
        option_tokens = options.iter().map(Vec::len).sum();
    }
    if option_tokens.saturating_add(8) > head_max {
        return Err(DecisionError::CardinalityLimit {
            question: question_id.into(),
            limit: head_max,
        });
    }
    head.truncate(head_max.saturating_sub(option_tokens).max(8));
    let mut ids = Vec::with_capacity(config.max_len);
    ids.push(special.cls);
    ids.extend(head);
    ids.push(special.sep);
    let mut markers = Vec::with_capacity(options.len());
    for option in options {
        markers.push(ids.len() as i64);
        ids.extend(option);
    }
    ids.push(special.sep);
    if ids.len() >= config.max_len {
        return Err(DecisionError::StateTooLong);
    }
    let room = config.max_len - ids.len() - 1;
    if newest_first {
        ids.extend_from_slice(&state_ids[state_ids.len().saturating_sub(room)..]);
    } else {
        ids.extend_from_slice(&state_ids[..room.min(state_ids.len())]);
    }
    ids.push(special.sep);
    Ok(Sequence { ids, markers, kind })
}

pub(super) fn answer(
    question: &Question,
    logits: &[f32],
    temperature: f64,
) -> Result<Answer, DecisionError> {
    if logits.is_empty() || logits.iter().any(|v| !v.is_finite()) {
        return Err(DecisionError::Internal);
    }
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64;
    let mut probabilities: Vec<f64> = logits
        .iter()
        .map(|v| ((f64::from(*v) - max) / temperature).exp())
        .collect();
    let sum: f64 = probabilities.iter().sum();
    for p in &mut probabilities {
        *p /= sum;
    }
    let best = probabilities
        .iter()
        .enumerate()
        .max_by(|(i, a), (j, b)| a.total_cmp(b).then_with(|| j.cmp(i)))
        .map(|(i, _)| i)
        .ok_or(DecisionError::Internal)?;
    let confidence = probabilities[best];
    Ok(match question {
        Question::Choice { criteria, .. } => Answer::Choice {
            choice: criteria
                .get_index(best)
                .ok_or(DecisionError::Internal)?
                .0
                .clone(),
            probabilities: criteria.keys().cloned().zip(probabilities).collect(),
            confidence,
        },
        Question::Score { .. } => Answer::Score {
            score: probabilities
                .iter()
                .enumerate()
                .map(|(i, p)| i as f64 * p)
                .sum(),
            probabilities,
            confidence,
        },
        Question::Noul { .. } => Answer::Noul {
            noul: probabilities[1],
            probabilities: Some(
                [
                    ("false".into(), probabilities[0]),
                    ("true".into(), probabilities[1]),
                ]
                .into(),
            ),
            confidence: Some(confidence),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn temperatures_respect_type_cardinality_and_clamp() {
        let cfg = Config {
            max_len: 512,
            head_max_len: 192,
            temperature: [2.0, 3.0, 4.0],
            temperature_by_options: [("choice:11+".into(), 0.1), ("noul:2".into(), 8.0)].into(),
        };
        assert_eq!(cfg.temperature(0, 20), 0.5);
        assert_eq!(cfg.temperature(1, 3), 3.0);
        assert_eq!(cfg.temperature(2, 2), 5.0);
    }
    #[test]
    fn probabilities_and_expected_score_use_calibration() {
        let q = Question::Score {
            instructions: String::new(),
            criteria: vec!["low".into(), "high".into()],
        };
        let Answer::Score {
            score,
            probabilities,
            confidence,
        } = answer(&q, &[0.0, 2.0], 2.0).unwrap()
        else {
            panic!()
        };
        assert!((score - 0.7310585786300049).abs() < 1e-12);
        assert_eq!(score, probabilities[1]);
        assert_eq!(confidence, score);
    }

    fn tokenizer() -> Tokenizer {
        Tokenizer::from_bytes(serde_json::to_vec(&serde_json::json!({
            "version": "1.0", "truncation": null, "padding": null,
            "added_tokens": [], "normalizer": null,
            "pre_tokenizer": {"type": "WhitespaceSplit"}, "post_processor": null, "decoder": null,
            "model": {"type": "WordLevel", "vocab": {"[UNK]": 0, "first": 1, "last": 2}, "unk_token": "[UNK]"}
        })).unwrap()).unwrap()
    }

    #[test]
    fn state_truncation_retains_start_or_newest_end_without_losing_options() {
        let tok = tokenizer();
        let special = SpecialIds {
            cls: 3,
            sep: 4,
            mask: 5,
            pad: 6,
            mask_text: "[MASK]".into(),
        };
        let config = Config {
            max_len: 32,
            head_max_len: 24,
            temperature: [1.0; 3],
            temperature_by_options: OrderedMap::new(),
        };
        let q = Question::Noul {
            instructions: "statement".into(),
            criteria: None,
        };
        let state = format!("first {} last", "filler ".repeat(100));
        let state_ids = encode(&tok, &special, &state).unwrap();
        let start = build(
            &tok,
            &special,
            &config,
            StateWindow {
                tokens: &state_ids,
                keep_newest: false,
            },
            "q",
            &q,
            false,
        )
        .unwrap();
        let end = build(
            &tok,
            &special,
            &config,
            StateWindow {
                tokens: &state_ids,
                keep_newest: true,
            },
            "q",
            &q,
            false,
        )
        .unwrap();
        assert!(start.ids.contains(&1));
        assert!(!start.ids.contains(&2));
        assert!(!end.ids.contains(&1));
        assert!(end.ids.contains(&2));
        assert_eq!(start.markers, end.markers);
        assert_eq!(
            start
                .markers
                .iter()
                .map(|m| start.ids[*m as usize])
                .collect::<Vec<_>>(),
            [5, 5]
        );
        assert_eq!(end.ids.len(), 32);
    }

    #[test]
    fn large_choice_sets_require_opt_in_and_still_fit_the_head() {
        let tok = tokenizer();
        let special = SpecialIds {
            cls: 3,
            sep: 4,
            mask: 5,
            pad: 6,
            mask_text: "[MASK]".into(),
        };
        let config = Config {
            max_len: 512,
            head_max_len: 192,
            temperature: [1.0; 3],
            temperature_by_options: OrderedMap::new(),
        };
        let question = |count| Question::Choice {
            instructions: "select".into(),
            criteria: (0..count).map(|i| (format!("option{i}"), None)).collect(),
        };
        assert!(matches!(
            build(
                &tok,
                &special,
                &config,
                StateWindow {
                    tokens: &[],
                    keep_newest: false
                },
                "q",
                &question(21),
                false
            ),
            Err(DecisionError::CardinalityLimit { limit: 20, .. })
        ));
        let sequence = build(
            &tok,
            &special,
            &config,
            StateWindow {
                tokens: &[],
                keep_newest: false,
            },
            "q",
            &question(21),
            true,
        )
        .unwrap();
        assert_eq!(sequence.markers.len(), 21);
        assert!(matches!(
            build(
                &tok,
                &special,
                &config,
                StateWindow {
                    tokens: &[],
                    keep_newest: false
                },
                "q",
                &question(100),
                true
            ),
            Err(DecisionError::CardinalityLimit { limit: 192, .. })
        ));
    }
}
