//! User settings and lexicon actions; input handling never waits for disk writes.
use super::*;
use crate::UserLink;
use twime_proto::{LexEntry, ToggleKey, UserReq};

impl Engine {
    pub fn with_user(
        assets: Arc<Assets>,
        cfg: Config,
        user: Option<UserLink>,
        legacy_toggle: ToggleKey,
    ) -> Arc<Engine> {
        Arc::new(Engine {
            assets,
            cfg,
            user,
            legacy_toggle,
        })
    }

    pub fn toggle_key(&self) -> ToggleKey {
        self.user().map_or(self.legacy_toggle, |u| u.service.settings().1.toggle_key)
    }

    pub fn user(&self) -> Option<&UserLink> {
        self.user.as_ref()
    }
}

impl Session {
    pub(super) fn initial_user_settings(engine: &Engine) -> (u64, bool, bool) {
        match engine.user() {
            Some(user) => {
                let s = user.service.settings();
                (s.0, s.1.punct_half, s.1.symbol_table)
            }
            None => (0, engine.cfg.punct_half, engine.cfg.symbol_table),
        }
    }

    pub(super) fn default_english(&self) -> bool {
        self.engine
            .user()
            .map_or(self.engine.cfg.default_english, |u| {
                u.service.settings().1.default_mode == Mode::English
            })
    }

    pub(super) fn user_toggle(&self) -> ToggleKey {
        self.engine.toggle_key()
    }

    pub(super) fn sync_user_settings(&mut self) {
        let Some(user) = self.engine.user() else {
            return;
        };
        let s = user.service.settings();
        if s.0 != self.settings_gen {
            self.settings_gen = s.0;
            self.punct_half = s.1.punct_half;
            self.symbol_table = s.1.symbol_table;
        }
        if self.err_gen.is_some_and(|g| user.service.error_gen() > g) {
            self.notice = Some("詞庫寫入失敗".into());
            self.err_gen = None;
        }
    }

    pub(super) fn add_to_lexicon(&mut self) -> bool {
        // Pre-v3 sessions keep their original no-op behaviour.
        if !self.ux {
            return false;
        }
        let reason = if self.engine.user().is_none() {
            Some("詞庫未啟用")
        } else if !self.st.run.is_empty() {
            Some("英文片段無法加入詞庫")
        } else if self.has_lit() {
            Some("含注音原樣，無法加入詞庫")
        } else if self.st.symbols {
            Some("符號表無法加入詞庫")
        } else if self.st.syl.len() > 8 {
            Some("最多 8 字")
        } else if !self.committable() {
            Some("組字未完成")
        } else {
            None
        };
        if let Some(reason) = reason {
            self.notice = Some(reason.into());
            return false;
        }
        let Some(word) = self.st.conv.as_ref().and_then(Conversion::text) else {
            self.notice = Some("組字未完成".into());
            return false;
        };
        let entry = LexEntry {
            word,
            // FUZZY 3.3: the effective syllables (the typed ones unless a repair converted this composition)
            reading: self.eff_syl().join(" "),
            weight: None,
        };
        if twime_userdata::validate(&self.engine.assets, &entry).is_err() {
            self.notice = Some("字與讀音不合".into());
            return false;
        }
        let user = self.engine.user().unwrap();
        // Capture before enqueue: a fast writer failure must also be observed.
        self.err_gen = Some(user.service.error_gen());
        self.notice = Some(format!("已送出加入詞庫：{}", entry.word));
        user.writer.submit_nowait(UserReq::Add(vec![entry]));
        false
    }
}

#[cfg(test)]
#[path = "user_tests.rs"]
mod tests;
