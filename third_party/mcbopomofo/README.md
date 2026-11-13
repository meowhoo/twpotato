# McBopomofo dictionary data

| File | Source | License |
| - | - | - |
| `data.txt` | [win-mcbopomofo](https://github.com/openvanilla/win-mcbopomofo) `data/data.txt`, commit `152bdde8c4a024addcca3dc687cd2837e954a195` (compiled McBopomofo dictionary) | MIT (McBopomofo); phrase entries derived from libtabe `tsi.src` (BSD) |
| `LICENSE-McBopomofo.txt` | [McBopomofo](https://github.com/openvanilla/McBopomofo) `LICENSE.txt`, commit `be6564acad6c4d3265c34a2e1a872d80f9db6068` | MIT |
| `COPYING-libtabe.txt` | libtabe 0.2.6 `COPYING` via [Debian sources](https://sources.debian.org/src/libtabe/0.2.6-1.2/COPYING/) | BSD 3-Clause (TaBE Project, Pai-Hsiang Hsiao 1999) and the other notices in the file |

The engine loads only pure-Han entries whose syllable count equals the
character count. The bpmfvs heterophony data
(Apache-2.0) used elsewhere in McBopomofo is not used here.

SHA-256:

```
3a4eb17a26ecb5bd3bcf4b15f7aec35659de48b4d31cac2b90a732b87b0c351b  data.txt
9cc138135c5fbfba9057868dc3055d154bbc3c936eea5e24d8a9dbb477a7efde  LICENSE-McBopomofo.txt
055d91064dfd275f7f098bd373cf04a2bf32ade93fc05b5dd3c3d7e798153c33  COPYING-libtabe.txt
```
