# Native palette inventory and provenance

DrakOps includes **28 palettes**: Learning Lab Navy, five DLMS palettes, and 22 bundled Omarchy v4.0.4 palettes. DLMS Ethereal and Omarchy Ethereal are distinct choices.

## Sources and notices

- **DLMS Learning Lab:** The renderer and palette source were adapted from [DLMS Learning Lab at 6b54e348](https://github.com/drakahari/dlms_learning_lab/tree/6b54e34869e031e50d6cf0854cce3402f65c6fd4). The original five DLMS values and source fingerprint are recorded in [dlms-stock.json](../assets/themes/dlms-stock.json). [LICENSE.dlms](../assets/themes/LICENSE.dlms) preserves the supplied MIT notice, copyright 2026 Mike Buchanan, for the inherited material.
- **Omarchy:** [Official v4.0.4](https://github.com/omacom/omarchy/releases/tag/v4.0.4), pinned to [c668141](https://github.com/omacom/omarchy/tree/c668141e9c42b13c80c9ca4ea108e11708c5e8a5). The 22 imported color files and their hashes are recorded in [omarchy-manifest.json](../assets/themes/omarchy-manifest.json); [LICENSE.omarchy](../assets/themes/LICENSE.omarchy) preserves the upstream MIT notice, copyright David Heinemeier Hansson.

These notices cover the applicable inherited material. Only theme code and color data are imported, not learning content, wallpapers, fonts, operating-system theming scripts, or host configuration.

## Inventory

| Group | Included picker names |
| --- | --- |
| Learning Lab | Learning Lab Navy |
| DLMS | Dark; Light; Purple & Gold; Maroon & Gold; Ethereal |
| Omarchy light | catppuccin-latte; flexoki-light; lupine; rose-pine; white |
| Omarchy dark | catppuccin; ethereal; everforest; gruvbox; hackerman; kanagawa; last-horizon; lumon; matte-black; miasma; nord; osaka-jade; retro-82; ristretto; solitude; tokyo-night; vantablack |

Use **Theme** in the DrakOps header. DrakOps stores the selected theme in its local device book. The desktop operating system is not changed. The renderer derives native colors from the source palettes and adjusts text and control contrast where needed. Palette checks are not a full accessibility audit.
