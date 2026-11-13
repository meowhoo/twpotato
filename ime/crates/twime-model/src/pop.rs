//! Optional AUTOCOMP-POP assets. Runtime inference uses the validated int8 S32 path.
use std::{collections::HashMap, io, path::Path, sync::Arc};
use sha2::{Digest, Sha256};
use crate::{autocomp::Completion, lm::{Lm, Workspace}};

pub struct CompletionLm {
    model: Lm,
    vocab: HashMap<char, u32>,
    pub weights_sha256: String,
    pub vocab_sha256: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_weights_disable_without_requiring_vocabulary() {
        let dir=std::env::temp_dir().join(format!("twime-pop-missing-{}",std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(CompletionLm::load_optional(&dir).unwrap().is_none());
    }
}
impl CompletionLm {
    /// `lm-f32.bin` is quantized on load; input embeddings stay f32. Missing weights disable POP.
    pub fn load_optional(dir: &Path) -> io::Result<Option<Arc<Self>>> {
        let bytes = match std::fs::read(dir.join("lm-f32.bin")) {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let vb = std::fs::read(dir.join("lm-vocab.json"))?;
        let chars: Vec<String> = serde_json::from_slice(&vb).map_err(io::Error::other)?;
        let model = Lm::from_bytes(&bytes, true).map_err(io::Error::other)?;
        if chars.len() != model.vocab || chars.get(..3) != Some(&["<PAD>".to_string(), "<BOS>".to_string(), "<UNK>".to_string()]) {
            return Err(io::Error::other("LM vocab size or reserved ids"));
        }
        let mut vocab = HashMap::new();
        for (i, s) in chars.iter().enumerate().skip(3) {
            let mut cs = s.chars();
            let c = cs.next().ok_or_else(|| io::Error::other("LM empty token"))?;
            if cs.next().is_some() || vocab.insert(c, i as u32).is_some() { return Err(io::Error::other("LM invalid token")); }
        }
        Ok(Some(Arc::new(Self { model, vocab, weights_sha256: format!("{:x}", Sha256::digest(&bytes)), vocab_sha256: format!("{:x}", Sha256::digest(&vb)) })))
    }
    pub fn rank(&self, context: &str, mut items: Vec<Completion>) -> Result<Vec<Completion>, &'static str> {
        let mut h: Vec<u32> = context.chars().map(|c| *self.vocab.get(&c).unwrap_or(&2)).collect();
        if h.len() > 32 { h.drain(..h.len()-32); }
        if h.is_empty() { h.push(1); }
        let adds: Vec<Vec<u32>> = items.iter().map(|c| c.extra_chars.chars().map(|ch| *self.vocab.get(&ch).unwrap_or(&2)).collect()).collect();
        let scores = self.model.score(&h, &adds, &mut Workspace::default(), false)?;
        let mut ranked: Vec<_> = items.drain(..).zip(scores).collect();
        ranked.sort_by(|(a, sa), (b, sb)| b.user.cmp(&a.user).then(sb.total_cmp(sa))
            .then(b.score.total_cmp(&a.score)).then((a.k+a.extra.len()).cmp(&(b.k+b.extra.len())))
            .then(a.word.cmp(&b.word)).then(a.extra.join(" ").cmp(&b.extra.join(" "))).then(b.k.cmp(&a.k)));
        Ok(ranked.into_iter().map(|(c, _)| c).collect())
    }
}
