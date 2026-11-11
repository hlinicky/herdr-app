# Translations

Herdr's texts are written in English where they're used. Every other
language is one catalog here, `<code>.json`, from the English text to its
translation (`src/i18n.rs` reads it; settings.json's `language` picks it).

## In mods

```js
import { tr } from "herdr";

tr("Close tab")                                   // a text
tr("Renamed {machine} to {name}.", { machine, name }) // values go in, never glued around
n === 1 ? tr("{n} agent", { n }) : tr("{n} agents", { n }) // English picks its form
```

- Call `tr()` while rendering, not once at load: the language can change while
  the mod runs, and mods redraw when it does.
- Translate a whole sentence with its values in it. Don't build a sentence out
  of translated pieces: word order differs between languages.
- Keep English in data (ids, command names, values compared in code) and
  translate where it's shown: `tr(section.title)`.
- A text that comes from somewhere else (props a caller passes, a program's
  error, a file name) is shown as it is; the caller translates what it passes.
- Product and program names stay: Herdr, herdr-core, hc, git, ssh.

## In the catalog

- The key is the English text exactly as `tr()` gets it, `{values}` included.
- A missing key shows English, so a new text never shows empty.
- Plural forms are a list, picked by the value `n`. Slovak has three:
  `["{n} agent", "{n} agenti", "{n} agentov"]` for 1, 2–4, and 0 or 5 and more.
- A translation keeps every `{value}` of its key (a test checks it).

## Slovenčina (`sk.json`)

Tykanie, krátko a vecne ako anglický originál. Odborné IT pojmy a pojmy Herdru
ostávajú po anglicky:

| English | Slovensky |
|---|---|
| space, spaces | space, spaces („Nový space“, „v tomto space“) |
| tab, tabs | tab, taby |
| pane, panes | pane, panes |
| layout | layout |
| mod, mods | mod, mody |
| agent | agent, agenti |
| worktree, branch, commit, diff, git, ssh, shell, prompt, font, zoom, PATH | bez prekladu |
| terminal | terminál |
| machine | stroj |
| panel | panel |
| theme | téma |
| settings | nastavenia |
| keys, shortcuts | klávesy, skratky |
| command palette | paleta príkazov |
| feedback | spätná väzba |
| update | aktualizácia, aktualizovať |
| What's new | Čo je nové |
| working / done / blocked / idle | pracuje / hotovo / čaká na teba / nečinný |
| Copy, Paste, Open, Close, Rename, Remove, Delete, Cancel, Save | Kopírovať, Vložiť, Otvoriť, Zavrieť, Premenovať, Odstrániť, Vymazať, Zrušiť, Uložiť |
