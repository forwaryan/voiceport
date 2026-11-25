//! Local catalog presentation; never mutates the configured recognition language,
//! install state, or selected transcription source.
use super::{Definition, MODELS, ModelView};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Preview {
    All,
    Live,
    AfterStop,
}
impl Preview {
    pub fn from_index(index: isize) -> Self {
        match index {
            1 => Self::Live,
            2 => Self::AfterStop,
            _ => Self::All,
        }
    }
    fn matches(self, model: &Definition) -> bool {
        match self {
            Self::All => true,
            Self::Live => model.kind.streaming(),
            Self::AfterStop => !model.kind.streaming(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Order {
    Recommended,
    Smallest,
    Largest,
}
impl Order {
    pub fn from_index(index: isize) -> Self {
        match index {
            1 => Self::Smallest,
            2 => Self::Largest,
            _ => Self::Recommended,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Browse {
    pub language: &'static str,
    pub preview: Preview,
    pub order: Order,
    pub installed_only: bool,
}

impl Browse {
    pub fn positions(self, rows: &[ModelView]) -> Vec<Option<usize>> {
        self.project(MODELS, |index| rows[index].installed())
    }

    fn project(
        self,
        models: &[Definition],
        installed: impl Fn(usize) -> bool,
    ) -> Vec<Option<usize>> {
        let mut visible: Vec<_> = models
            .iter()
            .enumerate()
            .filter(|(index, model)| {
                (!self.installed_only || installed(*index))
                    && model.kind.supports_language(self.language)
                    && self.preview.matches(model)
            })
            .collect();
        visible.sort_by_key(|(index, model)| match self.order {
            Order::Recommended => (
                match model.id {
                    crate::local_models::SENSEVOICE => 0,
                    crate::local_models::WHISPER_TURBO => 1,
                    crate::local_models::PARAFORMER => 2,
                    _ => 3,
                },
                *index,
            ),
            Order::Smallest => (model.bytes(), *index),
            Order::Largest => (u64::MAX - model.bytes(), *index),
        });
        let mut positions = vec![None; models.len()];
        for (position, (index, _)) in visible.into_iter().enumerate() {
            positions[index] = Some(position);
        }
        positions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_preview_size_and_installation_are_independent_browse_axes() {
        let browse = Browse {
            language: "en",
            preview: Preview::Live,
            order: Order::Smallest,
            installed_only: false,
        };
        let positions = browse.project(MODELS, |_| false);
        let models = |positions: Vec<Option<usize>>| {
            let mut shown: Vec<_> = positions
                .into_iter()
                .enumerate()
                .filter_map(|(index, pos)| pos.map(|p| (p, &MODELS[index])))
                .collect();
            shown.sort_by_key(|x| x.0);
            shown
                .into_iter()
                .map(|(_, model)| model)
                .collect::<Vec<_>>()
        };
        let visible = models(positions);
        assert!(visible.len() >= 2);
        assert!(
            visible
                .iter()
                .all(|d| d.kind.supports_language("en") && d.kind.streaming())
        );
        assert!(visible.windows(2).all(|ds| ds[0].bytes() <= ds[1].bytes()));
        let reversed = models(
            Browse {
                order: Order::Largest,
                ..browse
            }
            .project(MODELS, |_| false),
        );
        assert!(reversed.windows(2).all(|ds| ds[0].bytes() >= ds[1].bytes()));
        assert!(
            models(
                Browse {
                    installed_only: true,
                    ..browse
                }
                .project(MODELS, |_| false)
            )
            .is_empty()
        );
        let subset = models(
            Browse {
                installed_only: true,
                ..browse
            }
            .project(MODELS, |idx| {
                MODELS[idx].id == crate::local_models::PARAFORMER
            }),
        );
        assert_eq!(
            subset.iter().map(|d| d.id).collect::<Vec<_>>(),
            [crate::local_models::PARAFORMER]
        );
        let yue = models(
            Browse {
                language: "yue",
                preview: Preview::AfterStop,
                ..browse
            }
            .project(MODELS, |_| false),
        );
        assert!(!yue.is_empty());
        assert!(
            yue.iter()
                .all(|d| d.kind.supports_language("yue") && !d.kind.streaming())
        );
        assert!(!yue.iter().any(|d| d.id == crate::local_models::WHISPER));
    }
}
