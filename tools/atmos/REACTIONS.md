# Газы и реакции BandaStation

Генерируется `python tools/atmos/catalog.py`; вручную не редактировать.

Формулы ниже описывают исполняемый DM-код. `r` — локальная скорость реакции,
`T` — K, `V` — литры, количества — mol, энергия — J. Пороговые значения включительно.
Если не указано иначе: Tnew=max((Told*Cold+ΔE)/Cnew,2.7) при Cnew>0.0003.

## Газы

| ID | DM type | C, J/(mol·K) | rarity |
| --- | --- | ---: | ---: |
| `o2` | `/datum/gas/oxygen` | 20 | 900 |
| `n2` | `/datum/gas/nitrogen` | 20 | 1000 |
| `co2` | `/datum/gas/carbon_dioxide` | 30 | 700 |
| `plasma` | `/datum/gas/plasma` | 200 | 800 |
| `water_vapor` | `/datum/gas/water_vapor` | 40 | 500 |
| `hypernoblium` | `/datum/gas/hypernoblium` | 2000 | 50 |
| `n2o` | `/datum/gas/nitrous_oxide` | 40 | 600 |
| `nitrium` | `/datum/gas/nitrium` | 10 | 1 |
| `tritium` | `/datum/gas/tritium` | 10 | 300 |
| `bz` | `/datum/gas/bz` | 20 | 400 |
| `pluoxium` | `/datum/gas/pluoxium` | 80 | 200 |
| `miasma` | `/datum/gas/miasma` | 20 | 250 |
| `freon` | `/datum/gas/freon` | 600 | 10 |
| `hydrogen` | `/datum/gas/hydrogen` | 15 | 600 |
| `healium` | `/datum/gas/healium` | 10 | 300 |
| `proto_nitrate` | `/datum/gas/proto_nitrate` | 30 | 200 |
| `zauker` | `/datum/gas/zauker` | 350 | 1 |
| `halon` | `/datum/gas/halon` | 175 | 300 |
| `helium` | `/datum/gas/helium` | 15 | 50 |
| `antinoblium` | `/datum/gas/antinoblium` | 1 | 1 |

## Порядок и подавление

Четыре фазы: 1 — предварительные, 2 — образование, 3 — распад/конденсация, 4 — горение.
Кандидаты собираются по major_gas (наименьшая rarity, при равенстве — первый реагент)
и порядку ключей moles **до** реакций. Поэтому важны даже нулевые ключи.
Температурные пороги проверяются по T в начале цикла; количества — по текущей смеси.
При hypernoblium≥5 и T>20 возвращается STOP_REACTIONS, если вообще найдены кандидаты.
Нельзя сортировать ключи или пересобирать кандидатов после каждого превращения.
reaction_results сбрасывается только после этих ранних выходов. GC вызывается при ненулевых флагах.

## Реакции

### `vapor`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:110](../../code/modules/atmospherics/gasmixtures/reactions.dm#L110).
- Режим: standard; фаза 3; major_gas `water_vapor`.
- Вход: water_vapor≥0.25; T≤303.15.
- Превращение: H2O → мокрый/замёрзший пол.
- Скорость: r=0.25; при T≤200 только при успешном freeze_turf.
- Энергия: T не меняется.
- Эффекты: freeze_turf, wet_floor, water_vapor_gas_act.
- Особенности: Без turf реакции нет; пермафрост может вернуть VOLATILE без расхода.

### `sterilization`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:152](../../code/modules/atmospherics/gasmixtures/reactions.dm#L152).
- Режим: standard; фаза 3; major_gas `miasma`.
- Вход: miasma≥0.01; T≥443.15.
- Превращение: miasma → O2.
- Скорость: r=min(miasma,20+(T−443.15)/20); доля H2O≤0.1.
- Энергия: ΔT=0.002r.
- Эффекты: нет.
- Особенности: Это прибавка к температуре, а не энергии.

### `plasmafire`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:196](../../code/modules/atmospherics/gasmixtures/reactions.dm#L196).
- Режим: standard; фаза 4; major_gas `plasma`.
- Вход: plasma≥0.01; o2≥0.01; T≥373.15.
- Превращение: plasma + q O2 → 0.75 CO2 + 0.25 H2O; при O2/plasma≥96 → tritium.
- Скорость: s=min((T−373.15)/1270,1); q=1.4−s; r=min((plasma либо O2/10)/9*s,plasma,O2/q).
- Энергия: +3000000r J.
- Эффекты: hotspot_expose.
- Особенности: Остатки топлива и O2 QUANTIZE; result=r(1+q).

### `h2fire`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:274](../../code/modules/atmospherics/gasmixtures/reactions.dm#L274).
- Режим: standard; фаза 4; major_gas `hydrogen`.
- Вход: hydrogen≥0.01; o2≥0.01; T≥373.15.
- Превращение: H2 + 0.5 O2 → H2O.
- Скорость: r=min(H2/2,O2/20,H2,2O2).
- Энергия: +2800000r J.
- Эффекты: hotspot_expose.
- Особенности: result=r; без нижнего clamp T.

### `tritfire`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:330](../../code/modules/atmospherics/gasmixtures/reactions.dm#L330).
- Режим: standard; фаза 4; major_gas `tritium`.
- Вход: tritium≥0.01; o2≥0.01; T≥373.15.
- Превращение: tritium + 0.5 O2 → H2O.
- Скорость: r=min(tritium/2,O2/20,tritium,2O2).
- Энергия: +2800000r J.
- Эффекты: radiation_pulse с вероятностью 10%, hotspot_expose.
- Особенности: Радиация: r>0.1 и E>2800000(V/2500)^3; range=min(sqrt(r)/0.5,20), threshold=0.3.

### `freonfire`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:395](../../code/modules/atmospherics/gasmixtures/reactions.dm#L395).
- Режим: standard; фаза 4; major_gas `freon`.
- Вход: o2≥0.01; freon≥0.01; T≥20; T≤283.
- Превращение: freon + q O2 → CO2.
- Скорость: s=0.5 при T<60, иначе (283−T)/263; q=1.4−s; r=min((freon либо O2/10)/4*s,freon,O2/q).
- Энергия: −300000r J.
- Эффекты: hot_ice (2%, 120<T<160, turf), hotspot_expose.
- Особенности: QUANTIZE остатков; result=r(1+q).

### `nitrousformation`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:469](../../code/modules/atmospherics/gasmixtures/reactions.dm#L469).
- Режим: standard; фаза 2; major_gas `bz`.
- Вход: o2≥10; n2≥20; bz≥5; T≥200; T≤250.
- Превращение: 0.5 O2 + N2 → N2O; BZ катализатор.
- Скорость: r=min(2O2,N2).
- Энергия: +10000r J.
- Эффекты: нет.
- Особенности: BZ≥5 не расходуется; комментарий endothermic неверен.

### `nitrous_decomp`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:509](../../code/modules/atmospherics/gasmixtures/reactions.dm#L509).
- Режим: standard; фаза 3; major_gas `n2o`.
- Вход: n2o≥0.02; T≥1400; T≤100000.
- Превращение: N2O → N2 + 0.5 O2.
- Скорость: r=N2O/2*T*(T−100000)/(−2500000000).
- Энергия: +200000r J.
- Эффекты: нет.
- Особенности: При T=100000 скорость нулевая; без нижнего clamp T.

### `bzformation`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:551](../../code/modules/atmospherics/gasmixtures/reactions.dm#L551).
- Режим: standard; фаза 2; major_gas `n2o`.
- Вход: n2o≥10; plasma≥10; T≤313.15.
- Превращение: 0.4 N2O + 0.8(1−d) plasma → (1−d) BZ + 0.4d N2 + 0.2d O2.
- Скорость: d=max(4*(plasma/(N2O+plasma)−0.75),0); r=min(0.01*min(N2O/plasma,1)*V/P,N2O/0.4,plasma/(0.8*(1−d))).
- Энергия: +r(80000+120000d) J.
- Эффекты: нет.
- Особенности: Давление P=n*8.31*T/V; result=r, а не количество BZ.

### `pluox_formation`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:613](../../code/modules/atmospherics/gasmixtures/reactions.dm#L613).
- Режим: standard; фаза 2; major_gas `tritium`.
- Вход: co2≥0.01; o2≥0.01; tritium≥0.01; T≥50; T≤273.15.
- Превращение: CO2 + 0.5 O2 + 0.01 tritium → pluoxium + 0.01 H2.
- Скорость: r=min(5,CO2,2O2,100tritium).
- Энергия: +250r J.
- Эффекты: нет.
- Особенности: Тритий расходуется.

### `nitrium_formation`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:660](../../code/modules/atmospherics/gasmixtures/reactions.dm#L660).
- Режим: standard; фаза 2; major_gas `tritium`.
- Вход: tritium≥20; n2≥10; bz≥5; T≥1500.
- Превращение: tritium + N2 + 0.05 BZ → nitrium.
- Скорость: r=min(T/2985.2,tritium,N2,20BZ).
- Энергия: −100000r J.
- Эффекты: нет.
- Особенности: BZ расходуется.

### `nitrium_decomp`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:706](../../code/modules/atmospherics/gasmixtures/reactions.dm#L706).
- Режим: standard; фаза 1; major_gas `nitrium`.
- Вход: o2≥0.01; nitrium≥0.01; T≤343.15.
- Превращение: nitrium → H2 + N2; O2 катализатор.
- Скорость: r=min(T/2985.2,nitrium).
- Энергия: +30000r J.
- Эффекты: нет.
- Особенности: O2 не расходуется.

### `freonformation`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:751](../../code/modules/atmospherics/gasmixtures/reactions.dm#L751).
- Режим: standard; фаза 2; major_gas `bz`.
- Вход: plasma≥0.06; co2≥0.03; bz≥0.01; T≥473.15.
- Превращение: 0.6 plasma + 0.3 CO2 + 0.1 BZ → freon.
- Скорость: m=min(plasma/0.6,BZ/0.1,CO2/0.3); h=e^(−((T−800)/200)^2)+3/(1+e^(−0.001(T−6000))); r=min(0.05hm,plasma/0.6,CO2/0.3,BZ/0.1).
- Энергия: −(7000/(1+e^(−0.0015(T−6000)))+1000)*0.1r J.
- Эффекты: нет.
- Особенности: e=NUM_E из DM, без замены на другую константу.

### `nobformation`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:804](../../code/modules/atmospherics/gasmixtures/reactions.dm#L804).
- Режим: standard; фаза 2; major_gas `tritium`.
- Вход: n2≥10; tritium≥5; T≥2.7; T≤15.
- Превращение: 10 N2 + 5d tritium → hypernoblium; BZ катализатор.
- Скорость: d=clamp(tritium/(tritium+BZ),0.001,1); r=min(0.01(N2+tritium),tritium/(5d),N2/10).
- Энергия: +20000000r/max(BZ,1) J.
- Эффекты: нет.
- Особенности: Валидация через QUANTIZE; BZ уменьшает расход трития и нагрев.

### `halon_o2removal`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:856](../../code/modules/atmospherics/gasmixtures/reactions.dm#L856).
- Режим: standard; фаза 1; major_gas `halon`.
- Вход: halon≥0.01; o2≥0.01; T≥343.15.
- Превращение: halon + 20 O2 → 2.5 pluoxium.
- Скорость: r=min(T/3731.5,halon,O2/20).
- Энергия: −2500r J.
- Эффекты: resin foam: open turf, r>0.99*343.15/3731.5, без foam/resin.
- Особенности: Код производит pluoxium, хотя комментарий и справочник говорят CO2; result=5r.

### `healium_formation`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:910](../../code/modules/atmospherics/gasmixtures/reactions.dm#L910).
- Режим: standard; фаза 2; major_gas `freon`.
- Вход: bz≥0.01; freon≥0.01; T≥25; T≤300.
- Превращение: 2.75 freon + 0.25 BZ → 3 healium.
- Скорость: r=min(0.3T,freon/2.75,4BZ).
- Энергия: +9000r J.
- Эффекты: нет.
- Особенности: result=3r.

### `zauker_formation`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:951](../../code/modules/atmospherics/gasmixtures/reactions.dm#L951).
- Режим: standard; фаза 2; major_gas `nitrium`.
- Вход: hypernoblium≥0.01; nitrium≥0.01; T≥50000; T≤75000.
- Превращение: 0.01 hypernoblium + 0.5 nitrium → 0.5 zauker.
- Скорость: r=min(0.000005T,100hypernoblium,2nitrium).
- Энергия: −5000r J.
- Эффекты: нет.
- Особенности: Реакция эндотермическая вопреки комментарию; result=0.5r. Подавляется ≥5 hypernoblium в общем react().

### `zauker_decomp`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:992](../../code/modules/atmospherics/gasmixtures/reactions.dm#L992).
- Режим: standard; фаза 3; major_gas `zauker`.
- Вход: n2≥0.01; zauker≥0.01.
- Превращение: zauker → 0.3 O2 + 0.7 N2; N2 катализатор.
- Скорость: r=min(20,N2,zauker).
- Энергия: +460r J.
- Эффекты: нет.
- Особенности: Исходный N2 ограничивает скорость, но не вычитается.

### `proto_nitrate_formation`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:1034](../../code/modules/atmospherics/gasmixtures/reactions.dm#L1034).
- Режим: standard; фаза 2; major_gas `pluoxium`.
- Вход: pluoxium≥0.01; hydrogen≥0.01; T≥5000; T≤10000.
- Превращение: 2 H2 + 0.2 pluoxium → 2.2 proto_nitrate.
- Скорость: r=min(0.005T,5pluoxium,H2/2).
- Энергия: +650r J.
- Эффекты: нет.
- Особенности: result=2.2r.

### `proto_nitrate_hydrogen_response`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:1074](../../code/modules/atmospherics/gasmixtures/reactions.dm#L1074).
- Режим: standard; фаза 1; major_gas `proto_nitrate`.
- Вход: proto_nitrate≥0.01; hydrogen≥150.
- Превращение: H2 → 0.5 proto_nitrate; proto_nitrate катализатор.
- Скорость: r=min(5,H2,proto_nitrate).
- Энергия: −2500r J.
- Эффекты: нет.
- Особенности: PN не расходуется, прирост 0.5r; result=0.5r.

### `proto_nitrate_tritium_response`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:1114](../../code/modules/atmospherics/gasmixtures/reactions.dm#L1114).
- Режим: standard; фаза 1; major_gas `proto_nitrate`.
- Вход: proto_nitrate≥0.01; tritium≥0.01; T≥150; T≤340.
- Превращение: tritium + 0.01 proto_nitrate → H2.
- Скорость: r=min(T/34*tritium*PN/(tritium+10PN),tritium,100PN).
- Энергия: +10000r J.
- Эффекты: radiation_pulse при E>10000(V/2500)^3 и location.
- Особенности: range=min(sqrt(r)/0.5,20), threshold=0.3; VOLATILE зависит от location.

### `proto_nitrate_bz_response`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:1167](../../code/modules/atmospherics/gasmixtures/reactions.dm#L1167).
- Режим: standard; фаза 1; major_gas `proto_nitrate`.
- Вход: proto_nitrate≥0.01; bz≥0.01; T≥260; T≤280.
- Превращение: BZ → 0.4 N2 + 1.6 He + 0.8 plasma; PN катализатор.
- Скорость: r=min(T/2240*BZ*PN/(BZ+PN),BZ,PN).
- Энергия: +60000r J.
- Эффекты: радиация, ядерные частицы, hallucination pulse при E>60000(V/2500)^3 и location.
- Особенности: PN не расходуется. Частицы=min(floor(r/5),6); range=min(sqrt(r−2.5particles)/1.5,20); галлюцинации 20r deciseconds.

### `antinoblium_replication`

- Источник: [code/modules/atmospherics/gasmixtures/reactions.dm:1222](../../code/modules/atmospherics/gasmixtures/reactions.dm#L1222).
- Режим: standard; фаза 2; major_gas `antinoblium`.
- Вход: antinoblium≥0.25; T≥20.
- Превращение: все прочие газы → antinoblium пропорционально составу.
- Скорость: r=min(antinoblium/90,n_other); если n_other<0.01 — поглотить весь остаток.
- Энергия: сохраняет T*C, clamp TCMB.
- Эффекты: нет.
- Особенности: При поглощении последнего остатка возвращает NO_REACTION, хотя смесь меняется.

### `h2o_conversion`

- Источник: [code/modules/atmospherics/machinery/components/electrolyzer/electrolyzer_reactions.dm:64](../../code/modules/atmospherics/machinery/components/electrolyzer/electrolyzer_reactions.dm#L64).
- Режим: electrolyzer; фаза 0; major_gas `water_vapor`.
- Вход: water_vapor≥0.01.
- Превращение: 2 H2O → O2 + 2 H2.
- Скорость: r=min(H2O/2,2.5*working_power^2).
- Энергия: сохраняет T*C, clamp TCMB.
- Эффекты: только вызов электролиза.
- Особенности: adjust_gas квантует каждое изменение и вызывает GC после каждого шага.

### `nob_conversion`

- Источник: [code/modules/atmospherics/machinery/components/electrolyzer/electrolyzer_reactions.dm:94](../../code/modules/atmospherics/machinery/components/electrolyzer/electrolyzer_reactions.dm#L94).
- Режим: electrolyzer; фаза 0; major_gas `hypernoblium`.
- Вход: hypernoblium≥0.01.
- Превращение: hypernoblium → antinoblium.
- Скорость: r=hypernoblium*clamp(SM_power−5000,0,4000)/4000; требуется SM_power>5000.
- Энергия: Cnew=Cold+r*(1−2000), Tnew=max(T*Cold/Cnew,TCMB).
- Эффекты: только электролиз от разряда SM.
- Особенности: Теплоёмкость вычисляется через неквантованное r; working_power не используется.

### `halon_generation`

- Источник: [code/modules/atmospherics/machinery/components/electrolyzer/electrolyzer_reactions.dm:125](../../code/modules/atmospherics/machinery/components/electrolyzer/electrolyzer_reactions.dm#L125).
- Режим: electrolyzer; фаза 0; major_gas `bz`.
- Вход: bz≥0.01.
- Превращение: BZ → 0.2 O2 + 2 halon.
- Скорость: r=min(BZ*(1−e^(−0.5*T*working_power/373.15)),BZ).
- Энергия: +91232.1r J.
- Эффекты: только вызов электролиза.
- Особенности: adjust_gas/GC по каждому газу; нет обычного reaction_results.
