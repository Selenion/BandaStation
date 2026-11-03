# Как Paradise связывает MILLA с игровыми эффектами

Полный разбор газообмена, тепла, конденсации, оборудования, памяти и границы
доступа — [PARADISE_ARCHITECTURE.md](PARADISE_ARCHITECTURE.md).

Проверено 2026-11-02 по текущему master Paradise:
`5aca8afbcf00ef1f2bb96e179bf1389289551689`. Исследование кода; Paradise не
собирался и не запускался. Наше подключение к SSair пока не изменено.

## Численный тик и передача результата

MILLA хранит газы, thermal_energy, hotspot_temperature, hotspot_volume и
fuel_burnt в Rust-структуре Tile. Тик работает с двумя буферами; z-уровни
обрабатываются рабочими потоками. После вычисления буферы переключаются,
а в DM сообщается о завершении тика. Модель не вызывает DM после каждой
реакции газа.

В post_process последовательно выполняются газовые реакции, обработка
hotspot, конденсация и формирование InterestingTile. Этот объект содержит
копию состояния тайла и флаги причин обработки: DISPLAY, HOT, WIND, WET, ICY.
При конденсации Rust уже вычитает воду, а DM затем делает пол скользким по
переданному флагу. Комментарий do_turf_effects прямо объясняет, что вызов DM
из рабочего потока заставил бы ждать основной поток на каждом таком тайле.

Источники:

- [Tile и InterestingTile](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/rust/src/milla/model.rs#L226).
- [Тик и переключение буферов](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/rust/src/milla/tick.rs#L11).
- [post_process](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/rust/src/milla/simulate.rs#L333).
- [Конденсация и флаги WET/ICY](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/rust/src/milla/simulate.rs#L740).
- [Уведомление о завершении тика](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/rust/src/milla/api.rs#L830).

## Огонь в DM

SSair.process_interesting_tiles читает данные MILLA. Для HOT он получает
temperature, fuel_burnt и параметры hotspot. Если объекта ещё нет и
fuel_burnt≥0.001, тайл попадает в список hotspots. Затем update_hotspot
создаёт/обновляет объект и поддерживает его по fuel_burnt. В этом пути нет
проверки текущего O2<0.5, используемой BandaStation в hotspot_expose.

Внешние источники нагрева используют другой путь: hotspot_expose создаёт
milla_safe-задачу, которая вызывает create_hotspot в синхронной фазе MILLA.
Результаты расчёта и внешние изменения разделены по фазам, чтобы DM не писал
в атмосферу одновременно с рабочим тиком Rust.

Трубы и атмосферное оборудование по-прежнему обрабатываются DM-процедурами
SSair. В газовых смесях Paradise также сохраняется DM react(); Rust-цикл
реакций тайлов не является заменой каждой операции с газами во всей игре.

Источники:

- [Применение InterestingTile](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/controllers/subsystem/SSair.dm#L408).
- [update_hotspot](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/game/turfs/turf.dm#L772).
- [Внешний hotspot_expose](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/modules/atmospherics/environmental/LINDA_fire.dm#L12).
- [Синхронные задачи](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/controllers/subsystem/SSair.dm#L759).
- [DM gas_mixture.react](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/modules/atmospherics/gasmixtures/gas_mixture.dm#L697).

## Замена пола и идентичность атмосферной ячейки

Повторно проверено 2026-11-03: master Paradise остаётся
`5aca8afbcf00ef1f2bb96e179bf1389289551689`, master tg —
`4b1be84802ffd6793c95f1b49108ba037aa07320`. Проверены текущие исходники,
Paradise локально не запускался. Файлы и контрольные суммы сохранены в
`tmp/atmos-turf-upstream/`.

В tg `/turf/open/ChangeTurf` при `CHANGETURF_INHERIT_AIR` и новом открытом
тайле сохраняет копию газа, `excited` и ссылку `excited_group`, после замены
восстанавливает их. Сам datum группы сохраняется, кулдауны не обнуляются.
Текущий файл `change_turf.dm` tg побайтно совпадает с локальным Banda.
[Путь tg](https://github.com/tgstation/tgstation/blob/4b1be84802ffd6793c95f1b49108ba037aa07320/code/game/turfs/change_turf.dm#L212).

В Paradise Rust `Tile` находится в постоянной ячейке массива z-уровня,
индексируемой координатами x/y. Замена DM-turf не удаляет элемент этого массива
и не создаёт новый численный ID. В рабочем решателе MILLA нет механизма
tg excited groups с breakdown/dismantle-кулдаунами: набор активных клеток
создаётся для итераций решения газообмена. Поле `excited_group` в DM-классе
`/turf/simulated` ещё объявлено, но это не перенос групп в MILLA.
[Ячейки](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/rust/src/milla/model.rs#L421),
[решатель](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/rust/src/milla/simulate.rs#L137).

Paradise `AfterChange` вызывает `initialize_milla` и пересчёт связности,
которые передаются через `milla_safe`. Инициализация обновляет режим,
температуру и свойства уже существующей координатной ячейки. Это не обещание
неизменного состава: `/turf/simulated/Initialize` задаёт начальную смесь,
а `AfterChange` при `ignore_air=FALSE` дополнительно ставит задачу усреднения
воздуха соседей. Этот путь не эквивалентен флагу наследования tg.
[Замена и усреднение](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/game/turfs/turf.dm#L407),
[инициализация](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/game/turfs/turf.dm#L718).

Следовательно, предложенный для Banda новый ID с переносом группы — наш
вариант адаптации tg-контракта, а не готовая реализация Paradise. У Paradise
можно взять разделение долгоживущей координатной ячейки и DM-объекта пола;
сохранение tg-групп и отклонение старых игровых эффектов всё равно требуют
отдельного решения в нашем порте. Пользователь выбрал постоянную ячейку по
координатам, сохранение tg-группы и таймеров и версию привязки к DM-полу.

## Отложенная инициализация и шаблоны карты

Повторно проверено 2026-11-03, текущий master имеет тот же SHA. Копии исходников
сохранены в `tmp/atmos-deferred-upstream/`. Paradise локально не запускался.

В Paradise `turf.Initialize` вызывает `initialize_milla`, а
`simulated.Initialize` задаёт начальную смесь через `blind_set_air(get_initial_air())`.
`AfterChange` обновляет режим/материал и связность через `milla_safe`.
Инициализация свойств не требует читать отдельный, уже созданный DM `air`:
она записывает свойства в существующую координатную ячейку Rust. Безопасные
задачи выполняются сразу, если MILLA простаивает, иначе ждут синхронной фазы.
Это не ожидание нашего tg-сигнала `COMSIG_ATOM_AFTER_SUCCESSFUL_INITIALIZE`.

Сам загрузчик Paradise создаёт тайлы через `ChangeTurf(defer_change = TRUE)`,
затем вызывает `AfterChange(TRUE, keep_cabling = TRUE)` для загруженного блока.
Начальные газы задаются в пути `Initialize`. В Banda сохраняется обычный
изменяемый DM `air` между синхронными расчётами, поэтому адаптер должен ждать
готовности именно этого объекта. Пользователь разрешил завершать привязку
по штатному сигналу успешной инициализации, сохраняя ячейку и её версию.

Источники: [инициализация газа](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/modules/atmospherics/environmental/LINDA_turf_tile.dm#L1),
[запись свойств ячейки](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/game/turfs/turf.dm#L718),
[загрузчик](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/modules/awaymissions/maploader/reader.dm#L175),
[безопасная очередь](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/controllers/subsystem/SSair.dm#L759).

## Значение для BandaStation

Наш предыдущий выбор из двух схем был неполным. У Paradise есть третий подход:
Rust рассчитывает состояние и последствия горения; DM материализует игровой
результат по переданным данным. Это отличается от отложенного воспроизведения
старого hotspot_expose, который заново принимает решение по конечной смеси.

В BandaStation hotspot — часть симуляции. Initialize вызывает perform_exposure,
а тот может взять образец газа, вызвать affected.react(src), вернуть газ в тайл
и вызвать fire_act у его содержимого. В Paradise DM hotspot в основном хранит
полученное состояние, обновляет представление и взаимодействует с игровыми
объектами; реакции тайла/hotspot рассчитывает Rust.

Следовательно, возможная адаптация подхода Paradise требует перенести или
согласовать всю изменяющую газ часть жизненного цикла hotspot. Просто передать
булев флаг «создать огонь» и вызвать существующий DM-конструктор недостаточно,
чтобы считать совместимость доказанной. Для радиации, заморозки, resin, частиц
и остальных эффектов наших 26 реакций также нужны отдельные проверки.

Рекомендация по итогам исследования: рассмотреть модель состояния и результатов
по образцу MILLA, сохранив наши формулы и явно определив границу DM-эффектов.
Пользователь позднее выбрал перенос расчёта среды по этому направлению.
Позднее пользователь также выбрал обратное влияние предметов между native-тиками,
как Paradise. Реализация и проверенные последствия описаны в [PHASES.md](PHASES.md).

Нагрузочная выгода и полная совместимость этого варианта пока не проверены.

## Как Paradise возвращает газ от предметов

Дополнительно проверен текущий master 2026-11-02; SHA не изменился:
`5aca8afbcf00ef1f2bb96e179bf1389289551689`.

В Paradise предметные обработчики сохраняются в DM. Конкретный пример —
`/obj/item/stack/sheet/mineral/plasma/fire_act`: вызывает `atmos_spawn_air`
с выделением `amount*10` газа плазмы и удаляет стопку. В Rust описание предмета
и эта процедура не переносятся.

Путь добавления газа:

1. `atmos_spawn_air` создаёт обычную DM-смесь и вызывает `blind_release_air`.
2. Та создаёт `/datum/milla_safe/turf_blind_release` и вызывает `invoke_async`.
3. Если native-тик идёт, `synchronize` ставит задачу в очередь. Если MILLA
   свободна, callback может исполниться сразу в безопасной фазе.
4. Callback получает `get_turf_air(T)` и делает `merge`. Изменённая привязанная
   смесь регистрируется для обратной записи.
5. `process_bound_mixtures` записывает dirty-смеси перед запуском следующего
   Rust-тика. Следующий расчёт уже видит добавленный газ.

Следовательно, влияние предмета применяется между native-тиками. Paradise
не прерывает уже идущую последовательность Rust-реакций, чтобы запустить
DM fire_act и продолжить ту же последовательность с новым газом.

Порядок обычного воздействия среды тоже отличается: в фазе HOT SSair вызывает
`temperature_expose` тайла и заинтересованных объектов. Конструктор hotspot
подключает обработчик входа; в нём нет нашего `perform_exposure` с повторной
реакцией образца и обходом `fire_act` всего содержимого. Имеющиеся fire_act
предметов, сигналы и реакции реагентов остаются игровыми DM-механизмами.

Источники:

- [Предмет выпускает газ](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/game/objects/items/stacks/sheets/mineral.dm#L350).
- [atmos_spawn_air → blind_release_air](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/modules/atmospherics/environmental/LINDA_system.dm#L104).
- [Callback добавления](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/game/turfs/turf.dm#L753).
- [Очередь и выполнение в безопасной фазе](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/controllers/subsystem/SSair.dm#L759).
- [Обратная запись перед следующим тиком](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/controllers/subsystem/SSair.dm#L304).
- [Воздействие HOT](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/controllers/subsystem/SSair.dm#L447).
- [Initialize hotspot](https://github.com/ParadiseSS13/Paradise/blob/5aca8afbcf00ef1f2bb96e179bf1389289551689/code/modules/atmospherics/environmental/LINDA_fire.dm#L77).

Для Banda это ещё один явно формулируемый вариант: сохранить предметы в DM
и применять их изменения между native-тиками, как Paradise. Он отличается
от сохранения исходных промежуточных точек Banda. Тест с углём уже показывает,
что простое откладывание fire_act меняет расход плазмы, поэтому такой порядок
должен быть отдельным решением пользователя и проверяться как изменение
поведения. Из текущего кода нельзя установить, насколько Paradise сохраняет
результаты собственной исторической LINDA; такой вывод здесь не делается.
