# Naming decision and research

**Selected 2026-10-08: Tosi**, formerly Task Runners, under the **Bottega**
marketplace. Bottega is a separate marketplace-only repository; the tools remain
in their own repositories. This decision is for personal use on two machines.
It does not reserve a crate name, publish a plugin, or merge the tools into one binary.

Research snapshot: **2026-10-08**. Zenta was the earlier front-runner, with Tosi
a strong contender; Daje and Annamo were also considered. The historical research,
registry checks, and existing-name overlaps below are retained for context.

## Historical shortlist

| Name | Verified meaning and limits | Naming fit |
| --- | --- | --- |
| **Zenta** | A Venetian nautical structural term: *dormiente dei bagli*, a longitudinal support for deck beams. The historical dictionary marks it “Venez.”; it does **not** establish “small squad” or “crew.” [Lingua del Mare](https://www.linguadelmare.it/entry?id=e248cc57-d04b-4d7f-a152-8b704783ce54) | Short, distinctive nautical association; interpreting it as support for workers is a metaphor. |
| **Tosi** | Plural of northern regional *toso*, boy/lad. University of Verona's AlpiLinK explicitly records **tósi** among Veneto words for children, varying by locality; stress falls on the first syllable. This is regional vocabulary, not uniquely Venetian-city usage or a technical word for a team. [AlpiLinK](https://alpilink.it/veneto/), [Treccani: toso](https://www.treccani.it/vocabolario/toso/) | Informal “the lads” association; also has an independent Finnish meaning below. |
| **Daje** | Romanesco encouragement: roughly “come on!” or “let's go!” in the intended use. Other contexts can express impatience or agreement, so this is not a single literal translation. [Treccani on encouragement](https://www.treccani.it/magazine/atlante/societa/Daje_apericena_e_altri_modi_di_dire_dell_italiano_pazzesco.html), [Vocabolario del romanesco contemporaneo, D entries](https://ricerca.sns.it/retrieve/23aacdb9-67ca-4391-b699-0a45e8888f42/247_D%27Achille%20et%20al_2021_VRC-D_postPrint.pdf) | Four-letter call to action; a natural metaphor for launching workers. Existing developer-tool use below. |
| **Annamo** | Romanesco form of *andiamo*: “we go” or, as an exhortation, “let's go.” Explicitly paired with *andiamo* in an Italian-language teaching exercise. [ALMA Edizioni, printed p. 12](https://www.almaedizioni.it/wp-content/uploads/2025/03/prima-unita.pdf#page=10), [Treccani on Romanesco verb forms](https://www.treccani.it/magazine/lingua_italiana/speciali/Pasolini/3_Perrone.html) | A collective “let's get going” association; longer than Daje, with a clear connection to coordinated work. |
| **Stormo** | Ordinary Italian: a flock of birds or insects in flight, and an operational aviation unit; older senses include an armed band. Not specifically Venetian. [Treccani](https://www.treccani.it/vocabolario/stormo/) | A coordinated-group metaphor with an aviation/military association. |

## Tosi in Finnish

Finnish **tosi** means **true, real, serious**. Kotus's *Suomen etymologinen
sanakirja* gives those senses as German *wahr, wirklich, ernst* in its word-family
entry. [Kotus dictionary](https://kaino.kotus.fi/suomenetymologinensanakirja/?etym_id=ETYM_12c564cbd33553e47ad1d575fe46bec9&list_id=48654&p=article&word=todenper%C3%A4inen)

This is also the stated connection for the existing company: Tosi's own rebrand
announcement explicitly identifies its name with Finnish “true.” The company's
etymology is therefore documented, not inferred from its location.
[Tosi announcement](https://blog.tosi.net/news/tosibox-evolves-to-tosi-marking-record-growth-and-innovation-milestones?hs_amp=true)

## Other candidates considered

| Name | Verified meaning and qualification |
| --- | --- |
| **Stolo** | Historical Italian *schiera, moltitudine*: band/company/multitude. The exact form is attested in a scholarly glossary citing GDLI XX, p. 425. “Venetian small fleet” was **not** established. [Bianchini, printed p. 170](https://www.examenapium.it/cs/biblio/Bianchini2019.pdf#page=170) |
| **Muda** | A historical Venetian merchant-galley convoy or the organized commercial convoy service. The convoy association is supported. [Treccani: maritime organization](https://www.treccani.it/enciclopedia/gli-armamenti-marittimi_%28Storia-di-Venezia%29/) |
| **Braza** | A historical Venetian/Padovan dictionary gives an armful: what the arms can hold at once. “Offshoot” or “specialized crew extension” was **not** established. [Patriarchi, printed p. 27](https://libsysdigi.library.illinois.edu/OCA/Books2012-06/vocabolariovenez00patr/vocabolariovenez00patr.pdf) |
| **Ciurma** | Crew, historically galley rowers; figuratively can mean a noisy or disreputable bunch. Ordinary Italian, not specifically Venetian. [Treccani](https://www.treccani.it/vocabolario/ciurma2/) |
| **Frotta** | A group or crowd moving together, commonly with an unordered implication. Ordinary Italian. [Treccani](https://www.treccani.it/vocabolario/frotta/) |
| **Torma** | A troop/crowd, historically a Roman cavalry unit; also used of animals. Ordinary Italian, with martial or disorderly associations. [Treccani](https://www.treccani.it/vocabolario/torma/) |
| **Raga** | A colloquial shortening of *ragazzi* and related forms, characteristic of youth language. Not specifically Venetian. [Treccani on shortenings](https://www.treccani.it/magazine/lingua_italiana/domande_e_risposte/grammatica/grammatica_357.html) |

## Registry snapshot and existing names

The live crates.io API was checked on **2026-10-08** using
`https://crates.io/api/v1/crates/<name>`. HTTP 404 means no existing crate was
returned at that time; it is **not a reservation** or a guarantee of future availability.

| crates.io result | Names |
| --- | --- |
| No existing crate (404) | `zenta`, `tosi`, `stolo`, `braza`, `ciurma`, `frotta`, `torma`, `stormo`, `daje`, `annamo` |
| Registered | [`muda`](https://crates.io/crates/muda) 0.21.2: menu utilities; [`leva`](https://crates.io/crates/leva) 0.4.3: LaTeX; [`raga`](https://crates.io/crates/raga) 0.0.0: event microservices; [`coro`](https://crates.io/crates/coro) 0.1.1: coroutines |

The added [`daje`](https://crates.io/api/v1/crates/daje) and
[`annamo`](https://crates.io/api/v1/crates/annamo) checks returned explicit
“crate … does not exist” JSON at **2026-10-08 23:42 UTC**. Initial requests
returned 403; requests with an identifying User-Agent obtained the registry's
404 responses. Neither name has been reserved or published.

Relevant existing technology names, separate from crate availability:

- **Tosi AI Connect** explicitly offers MCP integration, making it a close naming overlap for this project. [Product page](https://www.tosi.net/tosi-ai-connect)
- **Zenta Pulse** operates in cloud/AI technology. [Zenta about page](https://www.zenta.ai/about)
- **RagaAI** is an existing AI platform. [Documentation](https://docs.raga.ai/)
- **Daje** is already the name of a Go dotfiles installer, described as unfinished
  in its [primary README](https://github.com/Schroedinger-Hat/daje/blob/main/README.md).
- **ANNaMo** is an existing scientific RNA/DNA folding and assembly model.
  [Primary paper](https://arxiv.org/abs/2311.03317)

The selected project, binary, and plugin identity is `tosi`. Optional public
GitHub hosting remains separate from registry or plugin publication. Earlier
review records retain the Task Runners identity and paths of their reviewed snapshots.
