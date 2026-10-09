New localization keys from parallel work land here first, one file per branch
(`<branch>.json`: `{ "<English key>": { "en": "...", "zh-Hans": "...", "zh-Hant": "...", "ja": "...", "ko": "..." } }`).
The lead folds them into `locales/*.json` on merge, so branches never conflict on the big files.
