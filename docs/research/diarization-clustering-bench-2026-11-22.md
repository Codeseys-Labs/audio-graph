# Speaker clustering bench: TitaNet-small and CAM++

Research date: 2026-11-22. Research-only experiment; no product code or shipping dependency changed.

## Direct answer

**Offline meeting refinement: NME-SC-style spectral clustering is the strongest candidate in this small experiment; tuned average-linkage AHC is the simpler strong baseline.** With unknown k, spectral scored **5.75%** Hungarian confusion on TitaNet AMI embeddings versus **7.38%** for tuned average AHC and **34.38%** for tuned complete AHC. CAM++ spectral scored **3.38%**, versus average AHC **5.25%**. Average AHC nearly solved the synthetic read-speech sessions, but that result did not transfer perfectly to real meetings. These are observed clustering-only results, not a production-switch recommendation or DER claims; two meetings are insufficient to freeze meeting-domain thresholds.

**Online: retain the existing one-to-one, duration-gated registry and feed it locally pooled, clean speaker evidence.** The replica with locally clustered window embeddings scored **8.00%** TitaNet AMI confusion at the default similarity, versus **39.25%** when one short embedding was fed at a time. The LibriSpeech-dev-tuned similarity actually worsened meeting confusion to **12.00%**; CAM++ improved modestly from 7.25% to 6.63%. This supports pooling, meeting-domain calibration and offline revision, not an untested rewrite of `stabilize.rs`.

**e-means did install and run on Windows, but did not provide a consistent speaker-recovery advantage.** At the tested five-epoch CPU operating point it was much worse on the unbalanced synthetic sessions even with the same k; it sometimes improved over Lloyd on the tiny AMI check. Better/faster fixed-k distortion optimization does not solve unknown speaker count. This compares one seeded e-means fit with ten Lloyd restarts, not quality-matched restart budgets; it does not refute the paper's large-codebook speed results.

**Genie can cluster these embeddings, but is not a default recommendation.** Gini 0.5 with oracle k was excellent; a dev-tuned external hierarchy cut scored 0.28% synthetic TitaNet confusion but **18.25%** on AMI. On CAM++ the cut tied spectral's **3.38%** AMI result. Thus it is genuinely viable, but its inequality prior and count/cut calibration are model/domain dependent; it does not choose k by itself. Its implementation is **AGPL v3**, adding a shipping-license question absent from a research-only comparison.

**Speed is not the main selection constraint at the tested desktop/session sizes.** On one CPU thread, 1,000 TitaNet embeddings took 44 ms for average AHC or 645 ms for spectral NME; at 10,000, 4.32 s or 5.89 s respectively. e-means reduced the NME+fit path from Lloyd's 1.17 s to 0.68 s at 10,000, but did not consistently recover speakers better. Genie cosine clustering was slower, around 11–12 s. [INFERENCE] For offline refinement, accuracy, calibration, licensing, and pairwise-memory use matter more than these optimizer savings; none of this establishes a hard realtime guarantee on low-end devices.

All numbers in the following tables come from the executed script's result JSON, not paper tables. Fractions use **mean ± sample standard deviation**. Synthetic rows pool 18 test sessions (six speaker counts × three seeds); AMI rows pool two meetings × three initialization seeds. Spreads mix session difficulty and seeds, not independent-meeting confidence intervals.

## Quality results

### TitaNet-small: synthetic sessions

Purity and coverage are percentages. Δk is estimated minus true speaker count. Oracle rows are diagnostics only.

| Method | ARI | Purity % | Coverage % | Δk | Confusion % |
|---|---:|---:|---:|---:|---:|
| Average AHC, dev threshold | 0.9994 ± 0.0017 | 100.00 ± 0.00 | 99.90 ± 0.24 | +0.17 ± 0.38 | **0.10 ± 0.24** |
| Average AHC, 0.5 | 0.8901 ± 0.1498 | 100.00 ± 0.00 | 91.67 ± 7.62 | +7.28 ± 5.38 | 8.33 ± 7.62 |
| Complete AHC, dev threshold | 0.9989 ± 0.0030 | 100.00 ± 0.00 | 99.72 ± 0.89 | +0.17 ± 0.38 | 0.28 ± 0.89 |
| Complete AHC, current 0.5 | 0.3157 ± 0.1595 | 100.00 ± 0.00 | 44.97 ± 15.28 | +20.78 ± 8.36 | 55.03 ± 15.28 |
| Spectral, NME k | 0.9818 ± 0.0742 | 99.72 ± 1.18 | 98.99 ± 4.27 | 0.00 ± 0.34 | 1.28 ± 4.36 |
| Lloyd, NME k | 0.9629 ± 0.1058 | 99.44 ± 1.62 | 98.02 ± 5.77 | 0.00 ± 0.34 | 2.53 ± 6.60 |
| Lloyd, silhouette k | 0.9515 ± 0.1378 | 97.78 ± 6.24 | 98.75 ± 5.30 | −0.17 ± 0.51 | 3.47 ± 8.62 |
| Lloyd, oracle k | 0.9606 ± 0.1150 | 99.44 ± 1.62 | 97.81 ± 6.44 | 0.00 ± 0.00 | 2.74 ± 8.04 |
| e-means, NME k | 0.7977 ± 0.2259 | 93.33 ± 6.69 | 89.58 ± 11.74 | 0.00 ± 0.34 | 17.08 ± 17.43 |
| e-means, silhouette k | 0.7775 ± 0.2196 | 90.83 ± 7.95 | 90.45 ± 10.67 | −0.17 ± 0.51 | 18.72 ± 16.54 |
| e-means, oracle k | 0.7940 ± 0.2285 | 92.50 ± 6.75 | 89.38 ± 11.95 | 0.00 ± 0.00 | 18.12 ± 18.12 |
| Genie, NME k, Gini 0.5 | 0.9927 ± 0.0281 | 99.72 ± 1.18 | 99.76 ± 1.03 | 0.00 ± 0.34 | 0.52 ± 1.52 |
| Genie, dev hierarchy cut | 0.9996 ± 0.0015 | 100.00 ± 0.00 | 99.72 ± 1.18 | +0.11 ± 0.47 | 0.28 ± 1.18 |
| HDBSCAN | 0.8879 ± 0.3230 | 94.06 ± 16.07 | 99.90 ± 0.24 | −0.56 ± 1.69 | 5.94 ± 16.07 |
| Registry, default similarity | 0.9679 ± 0.0605 | 98.85 ± 1.90 | 98.75 ± 2.32 | +0.17 ± 0.51 | 1.35 ± 2.45 |
| Registry, dev similarity | 0.9708 ± 0.0518 | 98.85 ± 1.90 | 98.92 ± 1.82 | +0.06 ± 0.24 | 1.18 ± 1.99 |
| Registry, single-short-embedding input | 0.8687 ± 0.1782 | 100.00 ± 0.00 | 88.12 ± 11.81 | +11.06 ± 8.97 | 11.88 ± 11.81 |

### CAM++: synthetic sessions

| Method | ARI | Purity % | Coverage % | Δk | Confusion % |
|---|---:|---:|---:|---:|---:|
| Average AHC, dev threshold | 0.9980 ± 0.0055 | 99.65 ± 1.47 | 99.83 ± 0.29 | +0.22 ± 0.43 | **0.49 ± 1.46** |
| Average AHC, 0.5 | 0.7332 ± 0.2856 | 100.00 ± 0.00 | 74.76 ± 19.16 | +18.00 ± 9.60 | 25.24 ± 19.16 |
| Complete AHC, dev threshold | 0.9810 ± 0.0664 | 99.17 ± 2.32 | 99.10 ± 3.03 | 0.00 ± 0.34 | 1.67 ± 4.77 |
| Complete AHC, current 0.5 | 0.1691 ± 0.1036 | 100.00 ± 0.00 | 29.44 ± 10.65 | +37.00 ± 9.70 | 70.56 ± 10.65 |
| Spectral, NME k | 0.9797 ± 0.0726 | 99.31 ± 1.94 | 98.99 ± 4.12 | −0.06 ± 0.42 | 1.67 ± 4.40 |
| Lloyd, NME k | 0.9583 ± 0.1146 | 99.03 ± 2.77 | 97.57 ± 7.06 | −0.06 ± 0.42 | 3.33 ± 8.94 |
| Lloyd, silhouette k | 0.9571 ± 0.1161 | 95.52 ± 7.72 | 99.62 ± 1.23 | −0.56 ± 0.92 | 4.48 ± 7.72 |
| Lloyd, oracle k | 0.9805 ± 0.0777 | 99.62 ± 1.47 | 98.96 ± 4.27 | 0.00 ± 0.00 | 1.25 ± 5.15 |
| e-means, NME k | 0.8030 ± 0.2270 | 93.99 ± 6.97 | 89.62 ± 12.04 | −0.06 ± 0.42 | 16.35 ± 17.97 |
| e-means, silhouette k | 0.8125 ± 0.2220 | 91.49 ± 8.96 | 92.36 ± 11.13 | −0.56 ± 0.92 | 16.08 ± 18.09 |
| e-means, oracle k | 0.7933 ± 0.2401 | 93.89 ± 6.95 | 88.92 ± 12.83 | 0.00 ± 0.00 | 17.15 ± 19.12 |
| Genie, NME k, Gini 0.5 | 0.9903 ± 0.0199 | 99.10 ± 2.00 | 99.51 ± 1.22 | −0.06 ± 0.42 | 1.15 ± 2.15 |
| Genie, dev hierarchy cut | 0.9960 ± 0.0077 | 99.90 ± 0.24 | 99.24 ± 1.64 | +0.22 ± 0.55 | 0.76 ± 1.64 |
| HDBSCAN | 0.4443 ± 0.5102 | 73.12 ± 24.50 | 99.76 ± 0.61 | −2.83 ± 3.47 | 26.88 ± 24.50 |
| Registry, default similarity | 0.8908 ± 0.1228 | 96.08 ± 4.18 | 95.80 ± 4.31 | +0.56 ± 1.20 | 4.58 ± 4.73 |
| Registry, dev similarity | 0.8878 ± 0.1313 | 95.76 ± 4.78 | 96.32 ± 4.06 | +0.06 ± 0.42 | 4.34 ± 4.85 |
| Registry, single-short-embedding input | 0.7354 ± 0.2369 | 100.00 ± 0.00 | 74.13 ± 11.84 | +25.78 ± 11.20 | 25.87 ± 11.84 |

### AMI transfer: no meeting-set tuning

Each meeting supplied 400 windows and four reference speakers. Rows below pool the two meetings and three initialization seeds. AHC/Genie/registry/HDBSCAN are deterministic here; repeating them over seeds is not extra independent evidence.

| Embedder / method | ARI | Purity % | Coverage % | Δk | Confusion % |
|---|---:|---:|---:|---:|---:|
| TitaNet / average AHC tuned | 0.7682 ± 0.1803 | 98.00 ± 0.27 | 92.62 ± 3.15 | +5.50 ± 0.55 | 7.38 ± 3.15 |
| TitaNet / complete AHC tuned | 0.3751 ± 0.2392 | 97.12 ± 0.14 | 65.62 ± 9.17 | +4.50 ± 0.55 | 34.38 ± 9.17 |
| TitaNet / spectral NME | 0.7738 ± 0.2049 | 97.38 ± 0.96 | 95.00 ± 2.19 | 0.00 ± 1.10 | **5.75 ± 3.01** |
| TitaNet / Lloyd NME | 0.5370 ± 0.4066 | 94.04 ± 3.07 | 77.88 ± 17.08 | 0.00 ± 1.10 | 22.46 ± 17.45 |
| TitaNet / Lloyd silhouette | 0.6444 ± 0.3173 | 94.42 ± 3.50 | 90.50 ± 6.66 | −0.83 ± 1.33 | 10.54 ± 7.76 |
| TitaNet / Lloyd oracle | 0.5709 ± 0.4153 | 97.12 ± 1.00 | 79.29 ± 20.62 | 0.00 ± 0.00 | 21.00 ± 20.96 |
| TitaNet / e-means NME | 0.4373 ± 0.4032 | 92.79 ± 3.63 | 68.62 ± 21.57 | 0.00 ± 1.10 | 33.96 ± 22.68 |
| TitaNet / e-means silhouette | 0.6533 ± 0.3113 | 94.42 ± 3.50 | 91.00 ± 6.38 | −0.83 ± 1.33 | 10.00 ± 7.45 |
| TitaNet / e-means oracle | 0.5500 ± 0.4405 | 96.29 ± 2.62 | 77.33 ± 23.59 | 0.00 ± 0.00 | 22.96 ± 23.95 |
| TitaNet / Genie NME | 0.7208 ± 0.2382 | 97.38 ± 0.96 | 91.88 ± 0.68 | 0.00 ± 1.10 | 9.00 ± 1.64 |
| TitaNet / Genie hierarchy cut | 0.2394 ± 0.2622 | 81.75 ± 10.41 | 99.50 ± 0.55 | −2.00 ± 1.10 | 18.25 ± 10.41 |
| TitaNet / HDBSCAN | 0.1945 ± 0.2092 | 69.62 ± 22.59 | 98.00 ± 1.92 | −3.00 ± 0.00 | 30.38 ± 22.59 |
| TitaNet / registry tuned | 0.6250 ± 0.0014 | 90.62 ± 5.07 | 88.00 ± 3.01 | +2.50 ± 0.55 | 12.00 ± 3.01 |
| CAM++ / average AHC tuned | 0.8184 ± 0.1678 | 99.00 ± 0.27 | 94.75 ± 2.74 | +6.00 ± 2.19 | 5.25 ± 2.74 |
| CAM++ / complete AHC tuned | 0.5409 ± 0.3991 | 98.00 ± 0.00 | 76.75 ± 18.35 | +4.00 ± 1.10 | 23.37 ± 18.49 |
| CAM++ / spectral NME | 0.8578 ± 0.0977 | 97.50 ± 0.55 | 97.25 ± 0.82 | −0.50 ± 0.55 | **3.38 ± 1.51** |
| CAM++ / Lloyd NME | 0.5783 ± 0.4200 | 96.04 ± 1.91 | 77.21 ± 22.53 | −0.50 ± 0.55 | 23.62 ± 23.43 |
| CAM++ / Lloyd silhouette | 0.8061 ± 0.1743 | 94.79 ± 3.45 | 95.58 ± 1.90 | −0.67 ± 1.51 | 5.54 ± 3.10 |
| CAM++ / Lloyd oracle | 0.5717 ± 0.4275 | 97.38 ± 0.41 | 75.67 ± 24.20 | 0.00 ± 0.00 | 24.71 ± 24.61 |
| CAM++ / e-means NME | 0.7087 ± 0.2915 | 95.46 ± 3.55 | 89.42 ± 14.60 | −0.50 ± 0.55 | 12.58 ± 14.93 |
| CAM++ / e-means silhouette | 0.8185 ± 0.1201 | 95.67 ± 2.71 | 94.71 ± 3.12 | −0.67 ± 1.51 | 6.33 ± 3.11 |
| CAM++ / e-means oracle | 0.5518 ± 0.4210 | 95.67 ± 3.50 | 75.79 ± 24.35 | 0.00 ± 0.00 | 25.75 ± 23.33 |
| CAM++ / Genie NME | 0.8468 ± 0.1054 | 97.50 ± 0.55 | 97.38 ± 0.68 | −0.50 ± 0.55 | 3.38 ± 1.51 |
| CAM++ / Genie hierarchy cut | 0.8468 ± 0.1054 | 97.50 ± 0.55 | 97.38 ± 0.68 | −0.50 ± 0.55 | 3.38 ± 1.51 |
| CAM++ / HDBSCAN | 0.0940 ± 0.0976 | 69.38 ± 22.59 | 98.38 ± 0.96 | −3.00 ± 0.00 | 30.62 ± 22.59 |
| CAM++ / registry tuned | 0.8145 ± 0.0106 | 95.62 ± 1.78 | 93.62 ± 1.51 | +3.00 ± 0.00 | 6.63 ± 1.23 |

Meeting-level confusion, mean over three seeds, makes the tiny sample and transfer visible:

| Embedder / meeting | Average AHC | Complete AHC | Spectral NME | Genie cut | Registry tuned |
|---|---:|---:|---:|---:|---:|
| TitaNet / ES2004a | 4.50% | 26.00% | 3.00% | 27.75% | 14.75% |
| TitaNet / TS3003a | 10.25% | 42.75% | 8.50% | 8.75% | 9.25% |
| CAM++ / ES2004a | 2.75% | 6.50% | 2.00% | 2.00% | 7.75% |
| CAM++ / TS3003a | 7.75% | 40.25% | 4.75% | 4.75% | 5.50% |

ES2004a's sampled counts were **196/107/54/43** per reference speaker. TS3003a's were **365/26/6/3**: one speaker dominates 91.25% of these eligible long single-speaker windows. A one-cluster result can therefore appear deceptively acceptable on TS3003a confusion while missing three people. This is why ARI and Δk matter alongside confusion; no averaging method makes these rare-speaker windows a representative full-meeting evaluation.

### Genie Gini sensitivity with oracle k

This separates a usable hierarchy from the unknown-k problem. All oracle runs have Δk=0. Mean ± SD confusion:

| Gini guard | TitaNet synthetic | CAM++ synthetic | TitaNet AMI | CAM++ AMI |
|---|---:|---:|---:|---:|
| 0.1 | 21.56 ± 22.99% | 20.66 ± 21.99% | 34.00 ± 11.50% | 22.87 ± 3.70% |
| 0.3 | 5.42 ± 9.68% | 1.98 ± 5.42% | 8.25 ± 7.12% | 7.13 ± 5.61% |
| 0.5 | **0.00 ± 0.00%** | **0.28 ± 0.75%** | **6.12 ± 4.79%** | **4.13 ± 2.33%** |
| 0.7 | 0.66 ± 1.92% | 2.74 ± 4.07% | 19.50 ± 8.22% | 15.50 ± 12.60% |
| 1.0, guard off | 2.01 ± 5.90% | 4.06 ± 6.48% | 22.88 ± 16.02% | 23.00 ± 15.88% |

### Spread by true speaker count

Synthetic confusion %, mean ± SD over the **three seeds per configuration**, rather than only pooled spreads:

| TitaNet speakers | Average AHC | Complete AHC | Spectral NME | e-means NME | Genie cut | Registry tuned |
|---|---:|---:|---:|---:|---:|---:|
| 2 | 0.00 ± 0.00 | 0.00 ± 0.00 | 0.00 ± 0.00 | 0.00 ± 0.00 | 0.00 ± 0.00 | 0.00 ± 0.00 |
| 3 | 0.21 ± 0.36 | 0.21 ± 0.36 | 0.00 ± 0.00 | 0.00 ± 0.00 | 0.00 ± 0.00 | 2.92 ± 3.15 |
| 4 | 0.00 ± 0.00 | 0.00 ± 0.00 | 0.00 ± 0.00 | 22.50 ± 19.58 | 0.00 ± 0.00 | 0.00 ± 0.00 |
| 6 | 0.21 ± 0.36 | 1.25 ± 2.17 | 6.04 ± 10.46 | 20.83 ± 9.55 | 0.00 ± 0.00 | 0.00 ± 0.00 |
| 8 | 0.21 ± 0.36 | 0.21 ± 0.36 | 0.00 ± 0.00 | 20.62 ± 19.50 | 0.00 ± 0.00 | 3.33 ± 1.91 |
| 10 | 0.00 ± 0.00 | 0.00 ± 0.00 | 1.67 ± 2.89 | 38.54 ± 8.76 | 1.67 ± 2.89 | 0.83 ± 1.44 |

| CAM++ speakers | Average AHC | Complete AHC | Spectral NME | e-means NME | Genie cut | Registry tuned |
|---|---:|---:|---:|---:|---:|---:|
| 2 | 0.00 ± 0.00 | 0.00 ± 0.00 | 0.00 ± 0.00 | 0.00 ± 0.00 | 0.21 ± 0.36 | 5.00 ± 6.61 |
| 3 | 0.21 ± 0.36 | 1.25 ± 2.17 | 0.21 ± 0.36 | 0.21 ± 0.36 | 0.21 ± 0.36 | 6.67 ± 7.22 |
| 4 | 0.00 ± 0.00 | 0.00 ± 0.00 | 0.00 ± 0.00 | 9.38 ± 16.24 | 0.00 ± 0.00 | 1.25 ± 1.25 |
| 6 | 0.21 ± 0.36 | 0.00 ± 0.00 | 5.83 ± 10.10 | 22.29 ± 10.63 | 0.00 ± 0.00 | 2.50 ± 4.33 |
| 8 | 2.29 ± 3.44 | 8.75 ± 9.82 | 2.29 ± 3.97 | 33.12 ± 23.28 | 3.33 ± 2.82 | 8.12 ± 3.25 |
| 10 | 0.21 ± 0.36 | 0.00 ± 0.00 | 1.67 ± 2.89 | 33.12 ± 7.98 | 0.83 ± 1.44 | 2.50 ± 4.33 |

### Dev-selected values and extracted data

| Embedder | Dimensions | Average distance | Complete distance | Registry similarity | Genie Gini / distance cut |
|---|---:|---:|---:|---:|---:|
| TitaNet-small | 192 | 0.70 | 0.90 | 0.45 | 0.5 / 0.40 |
| CAM++ English VoxCeleb | **512** | 0.75 | 0.95 | 0.50 | 0.5 / 0.45 |

Both extractors processed the same **338 utterances / 3,057 windows / 24 speakers** (100–212 windows per speaker). Timed extraction, excluding model/audio download, took **76.41 s** for TitaNet and **88.34 s** for CAM++. Downloaded data plus both model assets totaled **204,493,973 bytes**. Exact model hashes:

- TitaNet-small (40,257,283 bytes): `ad4a1802485d8b34c722d2a9d04249662f2ece5d28a7a039063ca22f515a789e`.
- CAM++ (29,596,979 bytes): `98daf9bcf1c8abf3db374fec4c27a5eda7c58705bb3f180d057b085a72a88bb5`.

The dev and test IDs are recorded in `extraction-*.json`. Dev: 61, 121, 237, 260, 672, 908, 1089, 1188, 1221, 1284, 1320, 1580. Test: 5683, 6829, 6930, 7021, 7127, 7176, 7729, 8224, 8230, 8455, 8463, 8555.

These high dev-selected distance thresholds **are not recommended production defaults**: they are calibrated on this embedding/window/data protocol, while the worker's concatenated local-cluster embeddings differ. Complete-linkage threshold 0.5 overfragmented even the reference-clean windows; that does not by itself measure the product's segmentation-plus-clustering DER.

## Speed results

TitaNet 192-dimensional embeddings; wall seconds, **mean ± sample SD across three seeds**. All 117 final method/size/seed rows completed without timeout or remaining error. Unknown-k rows include the selected count estimator; oracle rows omit that cost and cannot be deployed as unknown-count methods.

| Method | 100 embeddings | 1,000 embeddings | 10,000 embeddings |
|---|---:|---:|---:|
| Average AHC tuned | 0.00093 ± 0.00006 | 0.0440 ± 0.0026 | 4.3234 ± 0.1168 |
| Complete AHC tuned | 0.00084 ± 0.00009 | 0.0481 ± 0.0030 | 4.3909 ± 0.2705 |
| Spectral NME | 0.0373 ± 0.0032 | 0.6452 ± 0.0468 | 5.8939 ± 0.0776 |
| Lloyd NME | 0.0368 ± 0.0047 | 0.6457 ± 0.0615 | 1.1675 ± 0.0699 |
| Lloyd silhouette | 0.2425 ± 0.0128 | 0.5237 ± 0.0254 | 1.1340 ± 0.2017 |
| Lloyd oracle | 0.0363 ± 0.0057 | 0.0952 ± 0.0091 | 0.6559 ± 0.0546 |
| e-means NME | 0.0189 ± 0.0056 | 0.5612 ± 0.0195 | 0.6776 ± 0.0300 |
| e-means silhouette | 0.2101 ± 0.0098 | 0.4371 ± 0.0158 | 0.6249 ± 0.0455 |
| e-means oracle | 0.0078 ± 0.0005 | 0.0251 ± 0.0007 | 0.1548 ± 0.0093 |
| Genie NME | 0.0141 ± 0.0011 | 0.6057 ± 0.0195 | 11.7190 ± 0.0423 |
| Genie hierarchy cut | 0.0031 ± 0.0004 | 0.0985 ± 0.0012 | 11.3726 ± 0.2345 |
| HDBSCAN cosine | 0.0032 ± 0.0004 | 0.0688 ± 0.0179 | 6.5557 ± 0.7315 |
| Registry, including local AHC | 0.0049 ± 0.0004 | 0.0377 ± 0.0060 | 0.4320 ± 0.0910 |

The subsecond NME cost at 1,000 can dominate the final fixed-k fit; swapping centroid optimizers does not remove it. Genie's generic cosine MST cost is visible at 10,000; its inequality correction is not a free speed upgrade over average AHC. Exact dense/condensed pairwise storage remains quadratic even when a run is fast: approximately 400 MB for a 10,000-vector f64 condensed distance matrix, 800 MB for a full square matrix, excluding scratch. Those byte counts are arithmetic, **not measured peak RSS**. Spectral avoids a full dense session affinity matrix here but still constructs neighbor edges from blockwise all-pairs comparisons.

## Product implications and executed checks

1. **Do not blindly transplant the synthetic thresholds into `clustering.rs`.** First validate a model-pinned, meeting-domain dev split using the worker's actual local-cluster audio/embedding construction. The existing knob controls complete-linkage distance, whereas the live registry knob controls cosine similarity in the opposite strictness direction. Average linkage or NME-SC requires access to extracted embeddings and a separate clustering path, not just setting sherpa's current threshold.
2. **Keep `stabilize.rs` as the live registry.** In this experiment local pooling substantially reduced short-embedding fragmentation. The registry cannot retroactively merge a mint caused by early weak evidence; the existing post-session refinement Seed `audio-graph-4379` is the right owner for revisions. `audio-graph-faaa` already owns clean exclusive embedding ranges and centroid snapshots; do not create a duplicate.
3. **Keep anonymous cluster recovery separate from person naming.** Neither lower e-means loss nor an accurate clustering partition provides a person's name, calibrated voiceprint confidence, consent, or cross-session identity.
4. **No new Faiss/Genie shipping dependency is justified by this bench.** The e-means result is a tested operating point, not evidence that large-scale fixed-k optimization is useless; Genie is viable but needs an external count/cut and license review. Rust candidates below are availability leads, not verified integration.

Observed execution:

- `prepare`: real sherpa inference completed for both models, 3,057 LibriSpeech embeddings each; all six final LibriSpeech/AMI banks contain finite vectors, with L2 norms between 0.99999988 and 1.00000012.
- `quality`: **396 result rows per embedder** (18 sessions × 22 methods); dev/test speakers disjoint.
- `ami`: **132 result rows per embedder** (two meetings × three seeds × 22 methods), 400 reference-exclusive windows per meeting.
- `timing --models titanet`: **117 final rows** (13 methods × three sizes × three seeds), no remaining failure or timeout.
- The first preparation attempt exposed a short reader with only 37 windows; the script now downloads additional utterances until all speakers have at least 100, rather than silently sampling repeated quality points. Initial Genie bootstrap cuts exposed negative cosine linkage heights, consistent with native floating-point roundoff on duplicate normalized vectors [INFERENCE]. The hierarchy adapter now applies the mathematically required nonnegative-distance clamp (as sherpa does for cosine distances); the two failed timing rows were rerun successfully. Before finalization, the two-speaker fixture was corrected from balanced talk time to the required 60/40 split; dev calibration, all quality scores, and affected AHC/registry timing rows were refreshed. The tables contain only the final protocol's results.
- Product Rust builds, tests, linters, formatters, full diarization DER, GPU e-means, and Rust clustering-library parity were **not run**; they are outside this research script's validation. No commit, push, tracker mutation, product-code edit, or shared configuration change was made.

## Question and baseline correction

Which clustering method recovers an **unknown** number of speakers from AudioGraph's real speaker embeddings, and is clustering speed a reason to change the live/offline architecture? This extends, rather than replaces, the [speaker identification research](speaker-identification-2026-11-21.md) and [late-binding identity design](../designs/lazy-speaker-identification.md). These labels represent anonymous voices, not named-person identification.

**The requested baseline needed a correction:** sherpa-onnx **1.13.4 uses complete linkage**, not average linkage, over `max(0, 1 − cosine)` and cuts the dendrogram at a distance threshold when `num_clusters <= 0`. Its tagged implementation is explicit about `HCLUST_METHOD_COMPLETE`. Average linkage remains a required comparison, but is a candidate algorithm change, not a faithful name for current FastClustering. [Pinned sherpa source](https://github.com/k2-fsa/sherpa-onnx/blob/142807252687d81b40d6315f23470a1512a00de3/sherpa-onnx/csrc/fast-clustering.cc).

## What was measured

The reproducible [script](../../scripts/research/diarization-clustering-bench/bench.py) and [protocol README](../../scripts/research/diarization-clustering-bench/README.md) define all grids, random seeds, timing boundaries, and metrics. They download public data and models only into ignored `data/` and `.cache/` directories.

- **Embedding models:** actual sherpa `SpeakerEmbeddingExtractor` inference, CPU, one thread, TitaNet-small from the same k2-fsa asset named in `src-tauri/src/models/mod.rs`, and English VoxCeleb CAM++. Not synthetic Gaussian vectors. Every embedding is a normalized 1.5 s audio window with 0.75 s hop. [Model release](https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models), [sherpa identification API](https://k2-fsa.github.io/sherpa/onnx/speaker-identification/index.html).
- **Read-speech sessions:** LibriSpeech test-clean (`clean/test` on HF). First/last 12 sorted speaker IDs form disjoint dev/test pools. Download the first 12 utterances per selected speaker and extend short speakers' utterance prefixes until each supplies at least 100 windows. Each session samples 160 windows without replacement, with 2/3/4/6/8/10 speakers. The two-speaker case uses **96/64 windows (60/40)**; with three or more, a dominant speaker contributes 80 windows and the rest share the other 80. Four-window turns are interleaved by seeded shuffle. Dev uses seeds 101/202; test uses 11/22/33: 12 dev and 18 test sessions per embedder. The same sessions are used for every method. The corpus contains read English, not spontaneous meeting dialogue. [Dataset card and CC BY 4.0 declaration](https://huggingface.co/datasets/openslr/librispeech_asr).
- **Meeting transfer:** AMI test meetings **ES2004a and TS3003a**, Mix-Headset. An event sweep of the pyannote `only_words` RTTMs discards overlap and retains reference single-speaker regions. Up to 400 windows, evenly distributed among valid windows over the entire meeting, are processed in time order. Hyperparameters are frozen from LibriSpeech dev, not tuned on these meetings. Three initialization seeds per meeting do **not** constitute six independent meetings. [Test list](https://github.com/pyannote/AMI-diarization-setup/blob/67c2d539286e89f68952d5dcf83912bd9f01dfae/lists/test.meetings.txt), [reference construction](https://github.com/pyannote/AMI-diarization-setup), [download source](https://github.com/pyannote/AMI-diarization-setup/blob/main/pyannote/download_ami_mini.sh), [AMI CC BY 4.0 license](https://groups.inf.ed.ac.uk/ami/corpus/license.shtml).
- **Hyperparameters:** average and complete AHC distance thresholds, registry similarity threshold, and Genie hierarchy distance/Gini settings minimize mean dev Hungarian confusion, independently for each embedder. Test sessions share no speakers, utterances, or overlapping windows with dev. Different test sessions can reuse source windows; consequently seed spreads are not independent-sample confidence intervals. Exact grids and chosen values are recorded by the script in `.cache/settings-*.json`.
- **Quality metrics:** adjusted Rand index (ARI); weighted cluster purity and reference-speaker coverage; predicted non-noise speaker count minus true count; and **one-to-one** Hungarian segment confusion. Extra clusters cannot all map to the same reference speaker: this deliberately penalizes fragmentation. Noise is wrong and excluded from purity/coverage numerators. These are segment-embedding metrics, **not DER**, not wall-clock speech coverage, and not named-speaker precision. The README specifies the formulas; implementation is in `metrics` in the script.
- **Scaling:** 100, 1,000, and 10,000 embeddings, three seeds, bootstrapped from real eight-speaker embeddings. Duplication is limited to this speed experiment. One hour at a 0.75 s hop is approximately 4,800 embeddings before speech filtering (3,600/0.75). Timers include distance construction, k selection, and final clustering, but exclude interpreter/import, extraction, and data I/O. Each timing job has a 180 s subprocess cap; reported timeouts are censored, never substituted with invented durations.

## Algorithm fidelity and exclusions

**NME-SC:** the NumPy/SciPy implementation follows the normalized maximum eigengap mechanism: binary p-nearest cosine-neighbor affinities, symmetrization, unnormalized graph Laplacian, and the ratio `(p/n)/(max eigengap/max eigenvalue)` to choose p and k. It examines 30 p candidates up to 25% of n, with k=1…12. NME analysis uses at most 512 uniformly spaced points; spectral assignment uses the full-session sparse graph and lowest-k eigenvectors plus Lloyd. This is **NME-SC-style**, not a benchmark of the complete NeMo pipeline: no multiscale affinity fusion, short-recording enhancement, majority vote, or connectivity repair. The paper's arXiv version appeared in March 2020 even though NeMo's source references the SPL work as 2019. [Paper](https://arxiv.org/abs/2003.02405), [NeMo primary implementation](https://github.com/NVIDIA/NeMo/blob/main/nemo/collections/asr/parts/utils/offline_clustering.py).

**k-means and e-means:** sklearn Lloyd uses k-means++ and ten restarts. Both receive the same NME-selected or cosine-silhouette-selected k; silhouette scans 2…12 on at most 512 points. Oracle-k is a diagnostic condition, not available to the unknown-count system and not a mathematical upper bound on all clustering methods. Faiss **1.16.0 installed successfully as a Windows wheel**, and the benchmark calls its actual `faiss.contrib.e_means.NumpyEmeans`, five epochs, assigning points to its returned final centroids. No look-alike implementation was substituted. Both objectives are squared Euclidean distance on normalized input embeddings, not spherical k-means. The e-means paper solves fixed-k distortion minimization; its training-step and batch scaling rules do not estimate speaker count. GPU/tensor kernels and large-codebook speed claims are not tested here. [Official e-means project](https://e-means.github.io/), [paper](https://arxiv.org/abs/2611.09501), [actual NumPy implementation](https://github.com/facebookresearch/faiss/tree/main/contrib/e_means), [Faiss PyPI](https://pypi.org/project/faiss-cpu/1.16.0/).

**Genie:** actual `genieclust` **1.3.0**, cosine metric, oracle-k at Gini 0.1/0.3/0.5/0.7/1.0, NME-estimated k, and a dev-tuned external distance cut of the returned hierarchy. Its Gini guard limits cluster-size inequality; it is **not an automatic k criterion**. At Gini 1.0 the guard is disabled, yielding single linkage for the original distance. A dominant speaker is an intentional test of this prior, not a reason to assume balanced clusters. Generic-metric MST construction is quadratic; the Genie merge pass's better bound must not be quoted as end-to-end cosine clustering complexity. The installed distribution is **GNU AGPL v3**, confirmed from package metadata, not a permissive runtime dependency. [Official manual](https://genieclust.gagolewski.com/), [API documentation](https://genieclust.gagolewski.com/pythonapi.html), [upstream release history](https://github.com/gagolews/genieclust/blob/master/NEWS), [PyPI 1.3.0](https://pypi.org/project/genieclust/1.3.0/).

**HDBSCAN:** sklearn 1.6.1, precomputed cosine distance, `min_cluster_size=5`, `min_samples=3`, `allow_single_cluster=True`; other defaults, no nearest-centroid rescue of rejected points. This is one explicit operating point, not an exhaustive density-clustering hyperparameter search. [Official API](https://scikit-learn.org/1.6/modules/generated/sklearn.cluster.HDBSCAN.html).

**Online registry:** the Python rule mirrors `stabilize.rs:102–166,200–253` for valid model embeddings: normalize locals; take a snapshot of all existing globals; sort qualifying cosine pairs descending with `(local, global)` ties; greedily assign one-to-one; mint unmatched locals; and update matching centroids only for active duration ≥1.5 s using a contribution-count-weighted running mean followed by normalization. Duration gates contributions but does **not weight** the mean. Default similarity is 0.6. Complete-linkage local clustering over successive eight-embedding batches feeds the multi-local registry, without oracle speaker labels. AMI batching retains time order but spans variable real durations because the valid reference windows are subsampled and contain gaps. A second comparison supplies one embedding/local per 1.5 s window. [Project source](../../src-tauri/src/diarization/stabilize.rs).

This is an exact registry assignment/update rule, **not an exact reproduction of the complete live worker**: the benchmark takes means of segment embeddings and derives local durations by union on a synthetic 0.75 s-hop grid; the worker instead runs neural diarization on overlapping rolling audio, embeds concatenated samples per local cluster, and emits the newly covered trailing hop. All represented locals have at least 1.5 s duration, so the experiment does not exercise the short-duration update rejection branch, despite implementing its gate. No invalid/zero embeddings appeared in the selected data, so invalid-input/sentinel behavior is not a runtime finding. [Worker](../../src-tauri/src/diarization/worker.rs), [identity design's registry context](../designs/lazy-speaker-identification.md#1-current-state-code-grounded-head-of-this-branch).

**VBx skipped, with a concrete reason:** the open BUT implementation requires a matching x-vector centering/LDA transform and a trained PLDA model (`--xvec-transform`, `--plda-file`). The downloaded TitaNet-small and CAM++ assets provide neither. Applying BUT's x-vector calibration to unrelated embedding spaces, or fitting on test speakers, would not be a meaningful VBx comparison. The open implementation was inspected; no generic smoothing stand-in is labeled VBx. This remains unmeasured, not a negative result for VBx. [BUT source](https://github.com/BUTSpeechFIT/VBx/blob/master/VBx/vbhmm.py), [VBx paper](https://arxiv.org/abs/2012.14952).

## Rust and sherpa integration options

The following are availability findings, **not** claims that these Rust implementations were compiled or validated in this experiment:

| Option | Primary-source availability | Integration consequence |
|---|---|---|
| Keep complete-linkage FastClustering | [sherpa 1.13.4](https://github.com/k2-fsa/sherpa-onnx/blob/v1.13.4/sherpa-onnx/csrc/fast-clustering.cc) | A calibrated complete-linkage distance threshold can stay inside the existing diarizer; no new clustering runtime. |
| Average-linkage AHC | [`kodama` 0.3.0](https://docs.rs/kodama/0.3.0/kodama/) accepts a condensed distance matrix and `Method::Average` | Changing sherpa's threshold does **not** change its linkage. Average linkage needs our clustering over sherpa-extracted embeddings, or an intentional upstream/fork change. Pairwise condensed f64 storage at n=10,000 is about 400 MB (10,000×9,999/2×8), before scratch. |
| Lloyd k-means | [`linfa-clustering` 0.8.1](https://docs.rs/linfa-clustering/0.8.1/linfa_clustering/) | Native Rust exists; still needs external auto-k and an embedding/timestamp extraction path. |
| HDBSCAN | [`petal-clustering` 0.14.0](https://docs.rs/petal-clustering/0.14.0/petal_clustering/) exposes `HDbscan` | Native Rust candidate exists; cosine and parity with sklearn's selected settings would need separate validation. |
| NME-SC | [NeMo reference](https://github.com/NVIDIA/NeMo/blob/main/nemo/collections/asr/parts/utils/offline_clustering.py) | Requires graph construction, eigensolver, auto-k and final assignment; it is not a selectable clustering mode in the pinned sherpa API. [INFERENCE] A Rust implementation is feasible but is extra numerical/backend work, not just a configuration change. |
| e-means | [`emeans` 0.1.1](https://docs.rs/emeans/0.1.1/emeans/) claims a CPU Faiss port; [`ruvector-e-means` 0.1.0](https://docs.rs/ruvector-e-means/0.1.0/ruvector_e_means/) documents an independent reference implementation; [crates.io search](https://crates.io/api/v1/crates?q=e-means&per_page=10) also finds `flow-clustering` | Rust implementations **do exist**, but are new and not validated here. [`faiss` 0.13.0](https://docs.rs/faiss/0.13.0/faiss/cluster/index.html) exposes conventional clustering/k-means; the existence of those bindings does not establish access to the Python-contrib e-means implementation. |
| Genie | [crates.io `genieclust` search](https://crates.io/api/v1/crates?q=genieclust&per_page=10) returned no crate; [`genie`](https://crates.io/crates/genie) is unrelated Age of Empires file handling | No verified drop-in Rust Genie binding was found. The upstream C++ core is AGPL v3: shipping requires license review or a clean paper-based reimplementation with legal review. This research dependency is not a product dependency. |

The retained registry is already std-only Rust. [INFERENCE] Keep online anonymous IDs stable while evaluating any offline method as revisable merge/split evidence; better offline clustering does not remove the late-binding ledger or justify rewriting transcript identities. This follows the existing [lazy identification design](../designs/lazy-speaker-identification.md), not a new identity architecture.

## Reproduction and evidence

From the repository root:

```sh
uv run --python 3.12 scripts/research/diarization-clustering-bench/bench.py prepare
uv run --python 3.12 scripts/research/diarization-clustering-bench/bench.py quality
uv run --python 3.12 scripts/research/diarization-clustering-bench/bench.py ami
uv run --python 3.12 scripts/research/diarization-clustering-bench/bench.py timing --models titanet
```

`all` also runs the speed experiment on CAM++, whereas the recorded speed comparison focuses on the shipped 192-dimensional TitaNet model. No global installs. Direct dependencies are pinned in the PEP 723 script header. Generated JSON under `.cache/` records individual sessions, fitted dev grids, exact environment, model hashes and extraction counts. See the README for cache invalidation, signed HF URL expiry, attribution, and precise metric definitions.

Environment observed: Windows 11 build 22631 x64; AMD64 Family 25 Model 97 Stepping 2; CPython 3.12.11; one native thread. NumPy 2.2.6, SciPy 1.15.3, scikit-learn 1.6.1, sherpa-onnx 1.13.4, soundfile 0.13.1, genieclust 1.3.0, faiss-cpu 1.16.0, requests 2.32.3, threadpoolctl 3.6.0. The direct-dependency pins are reproducible inputs, but this is not a complete transitive lockfile or hardware-independent timing guarantee.

## Limits on the decision

- Isolated clustering with reference single-speaker input omits VAD/segmentation mistakes, overlap handling, silence detection, missed/false speech, microphone channel effects, and final transcript alignment. LibriSpeech utterance labels do not assert every included sample is voiced; no internal VAD was applied. AMI Mix-Headset is not distant-microphone SDM.
- Two real meetings are a small transfer check, not evidence of broad meeting-domain superiority. Short 160-embedding synthetic sessions and a 12-speaker search cap do not test long unconstrained real sessions or guarantee robustness to a sole speaker.
- Windows overlap within an utterance, so embeddings are correlated. Test session windows can recur in different seeds/configurations. Means and spreads describe this experiment; they are not statistical significance claims.
- Only timing bootstraps reach 10,000 vectors. Those repeats can make graph structure and convergence unlike genuinely new acoustic observations. No 10,000-unique-window accuracy result is implied.
- The hand-sized NME-SC variant, one HDBSCAN operating point, five-epoch CPU reference e-means, and synthetic registry feeder do not cover all respective implementations or hyperparameters. A failed operating point is not a universal rejection of an algorithm family.
- Current release builds lacking a neural diarizer (existing Seed `audio-graph-8848`) are not fixed by any clustering benchmark. TitaNet-small checkpoint redistribution/license verification remains separate from the license of sherpa or NeMo code.
- Research used verified live source plus primary references; the machine's XERJ registry has no AudioGraph worktree entry. No shared index/configuration was changed.
