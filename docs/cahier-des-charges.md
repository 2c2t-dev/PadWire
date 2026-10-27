# PadWire — cahier des charges

Lecteur de covers Launchpad pour Linux. PadWire = Launch**Pad** + Pipe**Wire**.

> Nom : **PadWire**. Crate : `padwire`. App ID : `dev._2c2t.padwire` (domaine 2c2t.dev ; le `_` est obligatoire car un segment ne peut pas commencer par un chiffre).
> Document destiné à servir de base au MVP (Claude Code). Les points marqués **[à vérifier]** n'ont pas été confirmés sur du matériel ou des fichiers réels.

## 1. Vision

Application Linux native (GTK4 + libadwaita) qui joue des **covers Launchpad** sans Ableton :

- ouvre des **UniPack** (format de la communauté UniPad) et des **projets Ableton `.als`** de covers ;
- joue les sons avec une latence faible sur PipeWire ;
- pilote les LEDs d'un vrai Launchpad (Mini MK3 en priorité, puis X, Pro MK3, MK2, Pro) ;
- affiche une grille virtuelle qui reflète les LEDs et qu'on peut aussi jouer à la souris.

Constat de départ : aucun outil ne fait ça aujourd'hui. Sous Linux, la seule solution est Ableton sous Wine, avec un pont MIDI maison pour convertir les dispositions de notes entre modèles de Launchpad.

## 2. Stack

| Besoin | Choix | Notes |
|---|---|---|
| Langage | Rust (édition 2021+) | Même stack que Pipedeck |
| UI | `gtk4` + `libadwaita` (gtk4-rs) | `relm4` envisageable, pas obligatoire |
| Audio | `cpal` au départ, derrière un trait `AudioBackend` | Backend `pipewire` natif dans un second temps si la latence l'exige |
| Décodage | `symphonia` | wav, mp3, ogg, flac ; tout est décodé en f32 en mémoire au chargement |
| MIDI | `midir` (ALSA sequencer) | |
| Fichiers | `flate2` (gzip .als), `quick-xml`, `zip` (UniPack) | |
| Communication temps réel | `rtrb` ou `crossbeam-channel` | Jamais de verrou ni d'allocation dans le thread audio |
| Config | `serde` + `toml` dans `~/.config/padwire/` | |
| Distribution | Flatpak (plus tard) | Le MIDI dans Flatpak demande l'accès au séquenceur ALSA, à prévoir |

## 3. Architecture

```
src/
├── main.rs / app.rs        AdwApplication, fenêtres
├── model/                  modèle interne commun (indépendant du format source)
├── formats/
│   ├── unipack/            parseur UniPack -> model
│   └── als/                parseur .als -> model (+ résolution des chemins)
├── engine/
│   ├── audio.rs            mixeur, voix, trait AudioBackend
│   ├── led.rs              exécution des séquences LED (timeline)
│   └── midifx/             (v0.3) émulation des effets MIDI d'Ableton
├── device/
│   ├── mod.rs              détection et connexion des Launchpad
│   └── profiles/           un profil par modèle (dispositions, SysEx, LEDs)
└── ui/                     vues libadwaita
```

### Modèle interne (commun aux deux formats)

```rust
struct Project {
    title: String,
    author: Option<String>,
    size: (u8, u8),            // 8x8
    pages: Vec<Page>,          // "chains" en UniPack, Chain Selector en Ableton
}
struct Page {
    pads: HashMap<(u8, u8), Pad>,   // (x, y), 1-indexé, origine en bas à gauche
}
struct Pad {
    sounds: Vec<SoundRef>,     // file circulaire : chaque appui passe au son suivant
    leds: Vec<LedProgram>,     // file circulaire aussi
    loop_mode: LoopMode,       // Once | Times(n) | Infinite
    wormhole: Option<usize>,   // page vers laquelle basculer après lecture
    choke_group: Option<u8>,
    playback: PlaybackMode,    // OneShot | Gate (s'arrête au relâchement)
}
enum LedProgram {
    Sequence(Vec<LedEvent>),   // UniPack, ou rendu précalculé
    MidiFx(MidiFxGraph),       // v0.3 : effets MIDI émulés en temps réel
}
struct LedEvent { at_ms: u32, target: LedTarget, color: LedColor }
enum LedTarget { Grid(u8, u8), Side(u8), Top(u8) }
enum LedColor { Off, Palette(u8), Rgb(u8, u8, u8) }
```

Les deux formats sont convertis vers ce modèle, et le moteur ne connaît que lui.

## 4. Matériel : profils Launchpad

### Mode Programmer (Mini MK3, X, Pro MK3)

- **Grille** : note = `ligne * 10 + colonne`, avec ligne et colonne de 1 à 8 et l'origine en bas à gauche, soit 11 à 88.
- **Boutons ronds de droite** : messages CC `r * 10 + 9`, de 19 (en bas) à 89 (en haut). Valeur 127 à l'appui, 0 au relâchement.
- **Rangée du haut** : CC 91 à 98.
- **LEDs de la grille** : Note On sur le canal 1 = couleur fixe, canal 2 = clignotant, canal 3 = pulsation. La vélocité est l'index dans la palette 128 couleurs de Novation.
- **LEDs des boutons ronds** : Control Change sur le même numéro que le bouton.
- **Couleurs RGB arbitraires** : par SysEx (voir le Programmer's Reference Guide de chaque modèle).
- **Ports** : le Mini MK3 expose deux ports, `LPMiniMK3 DA` (DAW) et `LPMiniMK3 MI` (MIDI). Le mode Programmer passe par le port **MI**.

SysEx de passage en mode Programmer :

| Modèle | ID | Programmer | Retour au mode Live |
|---|---|---|---|
| Mini MK3 | `0D` | `F0 00 20 29 02 0D 0E 01 F7` | `F0 00 20 29 02 0D 0E 00 F7` |
| X | `0C` | `F0 00 20 29 02 0C 0E 01 F7` **[à vérifier]** | idem avec `00` |
| Pro MK3 | `0E` | `F0 00 20 29 02 0E 0E 01 F7` **[à vérifier]** | idem avec `00` |
| MK2 | `18` | changement de layout par SysEx **[à vérifier]** | |

La valeur du Mini MK3 vient du prototype de pont, mais l'effet du SysEx lui-même n'a pas été confirmé visuellement.

### Disposition « Drum Rack » (MK2 / Pro en mode User)

C'est la disposition de la plupart des anciennes covers Ableton.

- La grille est vue comme deux moitiés de 4 colonnes. Note = `36 + moitié * 32 + ligne * 4 + (colonne % 4)`, avec ligne et colonne en base 0 depuis le bas à gauche, et moitié = 0 pour la gauche, 1 pour la droite. Ça couvre les notes 36 à 99.
- Les boutons ronds de droite donnent les notes 100 à 107. Sens **[à vérifier]** : l'hypothèse actuelle est 100 en bas.
- Canal **6** (soit 5 en base 0) pour les notes et les LEDs.

Conversion bidirectionnelle validée par des tests aller-retour dans le prototype :

```python
def prog_to_drum(note):          # 11-88 -> 36-99
    row, col = note // 10 - 1, note % 10 - 1
    half = col // 4
    return 36 + half * 32 + row * 4 + (col % 4)

def drum_to_prog(note):          # 36-99 -> 11-88
    half, j = divmod(note - 36, 32)
    row, col = j // 4, j % 4 + 4 * half
    return (row + 1) * 10 + (col + 1)
```

### Palette

Il faut embarquer la table RGB des 128 couleurs de la palette Novation, pour la grille virtuelle et pour la commande `auto <vélocité>` des UniPack. Elle est commune aux modèles RGB récents **[à vérifier pour le MK2]**.

## 5. Format UniPack (v0.1)

Source : documentation officielle UniPad (unipad.io/docs/unipack).

Archive `.zip` (ou dossier) :

```
info          obligatoire   métadonnées
keySound      obligatoire   correspondance bouton -> son
sounds/       obligatoire   fichiers audio
keyLED/       optionnel     animations LED
autoPlay      optionnel     lecture automatique
```

Règles générales : noms de fichiers insensibles à la casse, UTF-8, coordonnées 1-indexées, temps en millisecondes.

**`info`** : lignes `clé=valeur`. Obligatoires : `title`, `producerName`, `buttonX`, `buttonY`, `chain` (1 à 24). Optionnels : `squareButton` (booléen), `website`.

**`keySound`** : une ligne par son, `chain x y fichier [loop] [wormhole]`.
- `loop` : absent ou `1` = une fois, `0` = en boucle infinie, `N ≥ 2` = N-1 répétitions.
- `wormhole` : page vers laquelle basculer après la lecture. Il est positionnel, donc il faut écrire `loop` (par exemple `1`) pour pouvoir le donner.
- Plusieurs lignes pour le même `chain x y` forment une **file circulaire** : chaque appui passe au son suivant.
- Les lignes de moins de 3 colonnes sont ignorées, et une coordonnée hors limites donne un avertissement.

**`keyLED/`** : un fichier par animation, nommé `chain x y [loop]` (`loop` vaut 1 par défaut, 0 = infini). Commandes, une par ligne :
- `on x y RRGGBB [vélocité]` ou `on x y auto <vélocité>` (abréviations : `o`, et `a` pour `auto`). La couleur hexadécimale sert à l'affichage dans l'app ; la vélocité est envoyée au Launchpad.
- `off x y` (abréviation `f`).
- `delay ms` (abréviation `d`).
- `chain n` (abréviation `c`).
- `*` ou `mc` en x désigne les boutons ronds. La numérotation des `mc` n'est pas documentée **[à vérifier sur des packs réels]**. `l` désigne le logo, qu'on ignore.

**`autoPlay`** : `on x y`, `off x y`, `touch x y`, `chain n`, `delay ms`. Les événements sans `delay` entre eux sont simultanés. `chain` remet à zéro les files de sons.

L'ordre des coordonnées est documenté comme x puis y. **[à vérifier sur plusieurs packs réels]** : des packs anciens pourraient utiliser ligne puis colonne.

## 6. Format Ableton `.als` (v0.2 et v0.3)

C'est du XML compressé en gzip. Il existe au moins deux générations de format :

- **Live 10 à 12** : `<FileRef>` contient `<RelativePathType>`, `<RelativePath Value="../Page 3/56.wav">` et `<Path Value="C:/Users/…/Page 3/56.wav">`, plus `OriginalFileSize` et `OriginalCrc`.
- **Live 9 et avant** : `<RelativePath>` contient une liste de `<RelativePathElement Dir="…"/>`, avec `<Name Value="56.wav"/>` et un `<SearchHint><PathHint>…`. **[structure exacte à vérifier]**

### Résolution des chemins de samples

Les covers sont distribuées avec les chemins du PC du créateur. Algorithme validé sur une cover réelle (561 chemins corrigés) :

1. indexer tous les fichiers du dossier de la cover par nom, en minuscules ;
2. pour chaque chemin d'origine, chercher le **plus long suffixe** de composants qui existe localement, par exemple `Faded/Page 3/56.wav`, puis `Page 3/56.wav` ;
3. accepter le nom de fichier seul uniquement s'il est unique. Dans les covers, les noms se répètent d'une page à l'autre : chaque dossier `Page N/` contient `1.wav`, `2.wav`, etc. ;
4. départager avec `OriginalFileSize` et `OriginalCrc` si nécessaire.

### Éléments à extraire

- **Drum Rack** (`DrumGroupDevice`) : branches, note de réception de chaque branche **[encodage à vérifier]**, et le Simpler ou Sampler de chaque branche (`OriginalSimpler` / `MultiSampler`) avec son `SampleRef`, le début et la fin du sample, le mode one-shot ou classique, le volume et les choke groups.
- **Pages** : Chain Selector du rack, avec la plage de sélection de chaque branche (`BranchSelectorRange` Min/Max **[nom à vérifier]**).
- **Mappings MIDI** (`KeyMidi` ou équivalent **[à vérifier]**) : canal, note ou CC, plage de notes. Exemple réel : « Notes E6:B6 » sur le canal 6, qui pilote le Chain Selector de 0 à 7 sur trois pistes. Avec la convention d'Ableton (C3 = 60), E6:B6 correspond aux notes 100 à 107.
- **Pistes** : MIDI From et MIDI To (port, canal) pour savoir quelle piste reçoit les pads et laquelle envoie les LEDs.
- **v0.3, rack d'effets MIDI** (`MidiEffectGroupDevice`) : chaînes avec zones de touches et de vélocité ; devices `MidiPitcher`, `MidiChord`, `MidiArpeggiator`, `MidiNoteLength`, `MidiVelocity`, `MidiScale`, `MidiRandom` **[noms XML à vérifier]**.
- **Hors périmètre au départ** : automation d'arrangement (changements de page synchronisés avec la timeline), clips MIDI de lumière, Max for Live.

### Cover de référence étudiée

« Alan Walker - Faded », par Hajime0512, créée pour un MK2 en mode User.

- Piste **Samples** : Drum Rack dont le Chain Selector gère environ 6 pages ; samples rangés dans `Page N/xx.wav`.
- Pistes **Lights** et **Glowing lights** : MIDI Effect Racks (Pitch, Chord, Arpeggiator) qui génèrent les LEDs dans la disposition Drum Rack, canal 6. Exemple capturé : un appui produit des notes 47, 51, 55… 67, soit une colonne qui monte, avec des vélocités autour de 94.
- Mappings : canal 6, notes 100 à 107 → Chain Selector des trois pistes.

## 7. Moteur audio

- Tous les samples d'un projet sont décodés et rééchantillonnés à la fréquence du backend au chargement. Aucun accès disque pendant le jeu.
- Mixeur à voix : 64 voix ou plus, vol des voix au-delà de la limite (voice stealing).
- Le thread audio reçoit des commandes (`Play{sample, gain}`, `Stop{voice}`, `Choke{group}`) par une file sans verrou.
- Latence cible : buffer réglable, 128 frames par défaut.

## 8. Moteur LED

- Exécute les `LedProgram` sur une horloge en millisecondes, dans un thread dédié hors thread audio.
- Gère plusieurs programmes simultanés, les boucles (`loop`) et l'arrêt des programmes au changement de page.
- Envoie les sorties au profil du Launchpad actif et à la grille virtuelle de l'UI.
- v0.3 : `MidiFx` reçoit les notes des pads, applique la chaîne d'effets (calée sur le tempo du projet) et produit des `LedEvent`.

## 9. Interface (libadwaita)

- **Bibliothèque** : liste ou grille des covers importées (titre, auteur, format), import par sélecteur de fichiers ou glisser-déposer (`.zip`, `.als` ou dossier).
- **Lecteur** : grande grille 8x8 qui reflète les LEDs et se joue à la souris, boutons ronds pour les pages, indicateur de page, état du Launchpad (modèle détecté, connecté ou non).
- **Préférences** (`AdwPreferencesWindow`) : périphérique MIDI, profil forcé, taille du buffer audio, sens des boutons de page, luminosité.
- **Toasts** (`AdwToast`) : samples manquants, Launchpad débranché, etc.

## 10. Feuille de route

| Version | Contenu | Critère d'acceptation |
|---|---|---|
| **v0.1** | Lecteur UniPack + Mini MK3 + grille virtuelle | Un UniPack réel se joue au Mini MK3 avec sons, LEDs et pages, sans décalage perceptible |
| **v0.2** | Import `.als` : sons, pages, mappings, résolution des chemins | La cover Faded se joue (sons et pages) sans Ableton |
| **v0.3** | Émulation des effets MIDI pour les LEDs des `.als` | Les LEDs de Faded correspondent à celles sous Ableton |
| **v0.4** | Autres Launchpad, autoPlay, mode entraînement, Flatpak | |

## 11. Tests

- Tests unitaires des conversions de disposition : aller-retour sur les 64 pads, comme dans le prototype.
- Fixtures UniPack et `.als` minimales dans `tests/fixtures/`. Ne pas committer de covers tierces, pour des raisons de droits.
- Port MIDI virtuel « faux Launchpad » pour tester sans matériel.

## 12. Références

- Prototype Python de pont MIDI (`lp_bridge.py`) et script de réparation des chemins (`fix_als_paths.py`), écrits pendant l'étude de la cover Faded.
- Documentation UniPack : https://unipad.io/docs/unipack
- Programmer's Reference Guide Novation (un par modèle)
- Code de l'app Android UniPad : https://github.com/kimjisub/unipad-android
- Parseurs `.als` existants, pour s'inspirer de la structure : https://github.com/abelsonlive/ableton
- Convertisseur `.als` → UniPack : https://github.com/MineEric64/UniConverter-Project
