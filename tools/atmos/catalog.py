"""Extract the current DM chemistry contract; deliberately not a general DM parser."""

import argparse
import ast
import hashlib
import json
import operator
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[2]
SOURCE_ROOT = ROOT
GASES = "code/modules/atmospherics/gasmixtures/gas_types.dm"
REACTIONS = "code/modules/atmospherics/gasmixtures/reactions.dm"
ELECTROLYZER = "code/modules/atmospherics/machinery/components/electrolyzer/electrolyzer_reactions.dm"
MIXTURE = "code/modules/atmospherics/gasmixtures/gas_mixture.dm"
DEFINES = [
    "code/__DEFINES/atmospherics/atmos_core.dm",
    "code/__DEFINES/atmospherics/atmos_gasses.dm",
    "code/__DEFINES/reactions.dm",
    "code/__DEFINES/supermatter.dm",
    "code/__DEFINES/maths.dm",
]

# Reviewed against executable react() bodies, not reaction_factors.dm descriptions.
# r is the rate variable of that reaction, not necessarily reaction_results.
RULES = {
    "vapor": ("H2O → мокрый/замёрзший пол", "r=0.25; при T≤200 только при успешном freeze_turf", "T не меняется", "freeze_turf, wet_floor, water_vapor_gas_act", "Без turf реакции нет; пермафрост может вернуть VOLATILE без расхода."),
    "sterilization": ("miasma → O2", "r=min(miasma,20+(T−443.15)/20); доля H2O≤0.1", "ΔT=0.002r", "нет", "Это прибавка к температуре, а не энергии."),
    "plasmafire": ("plasma + q O2 → 0.75 CO2 + 0.25 H2O; при O2/plasma≥96 → tritium", "s=min((T−373.15)/1270,1); q=1.4−s; r=min((plasma либо O2/10)/9*s,plasma,O2/q)", "+3000000r J", "hotspot_expose", "Остатки топлива и O2 QUANTIZE; result=r(1+q)."),
    "h2fire": ("H2 + 0.5 O2 → H2O", "r=min(H2/2,O2/20,H2,2O2)", "+2800000r J", "hotspot_expose", "result=r; без нижнего clamp T."),
    "tritfire": ("tritium + 0.5 O2 → H2O", "r=min(tritium/2,O2/20,tritium,2O2)", "+2800000r J", "radiation_pulse с вероятностью 10%, hotspot_expose", "Радиация: r>0.1 и E>2800000(V/2500)^3; range=min(sqrt(r)/0.5,20), threshold=0.3."),
    "freonfire": ("freon + q O2 → CO2", "s=0.5 при T<60, иначе (283−T)/263; q=1.4−s; r=min((freon либо O2/10)/4*s,freon,O2/q)", "−300000r J", "hot_ice (2%, 120<T<160, turf), hotspot_expose", "QUANTIZE остатков; result=r(1+q)."),
    "nitrousformation": ("0.5 O2 + N2 → N2O; BZ катализатор", "r=min(2O2,N2)", "+10000r J", "нет", "BZ≥5 не расходуется; комментарий endothermic неверен."),
    "nitrous_decomp": ("N2O → N2 + 0.5 O2", "r=N2O/2*T*(T−100000)/(−2500000000)", "+200000r J", "нет", "При T=100000 скорость нулевая; без нижнего clamp T."),
    "bzformation": ("0.4 N2O + 0.8(1−d) plasma → (1−d) BZ + 0.4d N2 + 0.2d O2", "d=max(4*(plasma/(N2O+plasma)−0.75),0); r=min(0.01*min(N2O/plasma,1)*V/P,N2O/0.4,plasma/(0.8*(1−d)))", "+r(80000+120000d) J", "нет", "Давление P=n*8.31*T/V; result=r, а не количество BZ."),
    "pluox_formation": ("CO2 + 0.5 O2 + 0.01 tritium → pluoxium + 0.01 H2", "r=min(5,CO2,2O2,100tritium)", "+250r J", "нет", "Тритий расходуется."),
    "nitrium_formation": ("tritium + N2 + 0.05 BZ → nitrium", "r=min(T/2985.2,tritium,N2,20BZ)", "−100000r J", "нет", "BZ расходуется."),
    "nitrium_decomp": ("nitrium → H2 + N2; O2 катализатор", "r=min(T/2985.2,nitrium)", "+30000r J", "нет", "O2 не расходуется."),
    "freonformation": ("0.6 plasma + 0.3 CO2 + 0.1 BZ → freon", "m=min(plasma/0.6,BZ/0.1,CO2/0.3); h=e^(−((T−800)/200)^2)+3/(1+e^(−0.001(T−6000))); r=min(0.05hm,plasma/0.6,CO2/0.3,BZ/0.1)", "−(7000/(1+e^(−0.0015(T−6000)))+1000)*0.1r J", "нет", "e=NUM_E из DM, без замены на другую константу."),
    "nobformation": ("10 N2 + 5d tritium → hypernoblium; BZ катализатор", "d=clamp(tritium/(tritium+BZ),0.001,1); r=min(0.01(N2+tritium),tritium/(5d),N2/10)", "+20000000r/max(BZ,1) J", "нет", "Валидация через QUANTIZE; BZ уменьшает расход трития и нагрев."),
    "halon_o2removal": ("halon + 20 O2 → 2.5 pluoxium", "r=min(T/3731.5,halon,O2/20)", "−2500r J", "resin foam: open turf, r>0.99*343.15/3731.5, без foam/resin", "Код производит pluoxium, хотя комментарий и справочник говорят CO2; result=5r."),
    "healium_formation": ("2.75 freon + 0.25 BZ → 3 healium", "r=min(0.3T,freon/2.75,4BZ)", "+9000r J", "нет", "result=3r."),
    "zauker_formation": ("0.01 hypernoblium + 0.5 nitrium → 0.5 zauker", "r=min(0.000005T,100hypernoblium,2nitrium)", "−5000r J", "нет", "Реакция эндотермическая вопреки комментарию; result=0.5r. Подавляется ≥5 hypernoblium в общем react()."),
    "zauker_decomp": ("zauker → 0.3 O2 + 0.7 N2; N2 катализатор", "r=min(20,N2,zauker)", "+460r J", "нет", "Исходный N2 ограничивает скорость, но не вычитается."),
    "proto_nitrate_formation": ("2 H2 + 0.2 pluoxium → 2.2 proto_nitrate", "r=min(0.005T,5pluoxium,H2/2)", "+650r J", "нет", "result=2.2r."),
    "proto_nitrate_hydrogen_response": ("H2 → 0.5 proto_nitrate; proto_nitrate катализатор", "r=min(5,H2,proto_nitrate)", "−2500r J", "нет", "PN не расходуется, прирост 0.5r; result=0.5r."),
    "proto_nitrate_tritium_response": ("tritium + 0.01 proto_nitrate → H2", "r=min(T/34*tritium*PN/(tritium+10PN),tritium,100PN)", "+10000r J", "radiation_pulse при E>10000(V/2500)^3 и location", "range=min(sqrt(r)/0.5,20), threshold=0.3; VOLATILE зависит от location."),
    "proto_nitrate_bz_response": ("BZ → 0.4 N2 + 1.6 He + 0.8 plasma; PN катализатор", "r=min(T/2240*BZ*PN/(BZ+PN),BZ,PN)", "+60000r J", "радиация, ядерные частицы, hallucination pulse при E>60000(V/2500)^3 и location", "PN не расходуется. Частицы=min(floor(r/5),6); range=min(sqrt(r−2.5particles)/1.5,20); галлюцинации 20r deciseconds."),
    "antinoblium_replication": ("все прочие газы → antinoblium пропорционально составу", "r=min(antinoblium/90,n_other); если n_other<0.01 — поглотить весь остаток", "сохраняет T*C, clamp TCMB", "нет", "При поглощении последнего остатка возвращает NO_REACTION, хотя смесь меняется."),
    "h2o_conversion": ("2 H2O → O2 + 2 H2", "r=min(H2O/2,2.5*working_power^2)", "сохраняет T*C, clamp TCMB", "только вызов электролиза", "adjust_gas квантует каждое изменение и вызывает GC после каждого шага."),
    "nob_conversion": ("hypernoblium → antinoblium", "r=hypernoblium*clamp(SM_power−5000,0,4000)/4000; требуется SM_power>5000", "Cnew=Cold+r*(1−2000), Tnew=max(T*Cold/Cnew,TCMB)", "только электролиз от разряда SM", "Теплоёмкость вычисляется через неквантованное r; working_power не используется."),
    "halon_generation": ("BZ → 0.2 O2 + 2 halon", "r=min(BZ*(1−e^(−0.5*T*working_power/373.15)),BZ)", "+91232.1r J", "только вызов электролиза", "adjust_gas/GC по каждому газу; нет обычного reaction_results."),
}


def read(path):
    root = SOURCE_ROOT if Path(path).parts[:1] == ("code",) else ROOT
    return (root / path).read_text(encoding="utf-8-sig")


def blocks(text):
    """Top-level absolute DM declarations only; fail later on unknown reactions."""
    matches = list(re.finditer(r"^(/[\w/]+)(?:\([^\n]*\))?[^\n]*", text, re.M))
    return {m[1]: text[m.start():matches[i + 1].start() if i + 1 < len(matches) else len(text)]
            for i, m in enumerate(matches)}


def constants():
    expressions = {"TRUE": "1", "FALSE": "0", "NONE": "0"}
    for path in DEFINES:
        text = re.sub(r"\\\n", "", read(path))
        expressions.update(dict(re.findall(r"^#define\s+(\w+)\s+([^\n]+)", text, re.M)))
    operations = {ast.Add: operator.add, ast.Sub: operator.sub, ast.Mult: operator.mul,
                  ast.Div: operator.truediv, ast.Pow: operator.pow, ast.LShift: operator.lshift}

    def evaluate(expression):
        def walk(node):
            if isinstance(node, ast.Constant) and isinstance(node.value, (int, float, str)):
                return node.value
            if isinstance(node, ast.Name):
                return evaluate(expressions[node.id])
            if isinstance(node, ast.UnaryOp) and isinstance(node.op, (ast.USub, ast.UAdd)):
                return (-1 if isinstance(node.op, ast.USub) else 1) * walk(node.operand)
            if isinstance(node, ast.BinOp) and type(node.op) in operations:
                return operations[type(node.op)](walk(node.left), walk(node.right))
            raise ValueError(f"Unsupported DM constant: {expression}")
        return walk(ast.parse(expression.split("//")[0].strip(), mode="eval").body)

    return expressions, evaluate


def extract():
    expressions, evaluate = constants()
    gases = []
    for path, body in blocks(read(GASES)).items():
        if not re.fullmatch(r"/datum/gas/\w+", path):
            continue
        props = dict(re.findall(r"^\t(id|specific_heat|rarity|name) = (.+)", body, re.M))
        gases.append(dict(path=path, key=path.rsplit("/", 1)[1],
                          **{k: evaluate(v) for k, v in props.items()}))
    assert len(gases) == evaluate("GAS_TYPE_COUNT")
    gas_by_path = {g["path"]: g for g in gases}
    reactions = []
    for source, kind in [(REACTIONS, "standard"), (ELECTROLYZER, "electrolyzer")]:
        declarations = blocks(read(source))
        for path, body in declarations.items():
            if not re.fullmatch(rf"/datum/gas_reaction/{kind}/\w+", path) or path.endswith("/New"):
                continue
            reaction_id = re.search(r'^\tid = "(.+)"', body, re.M)[1]
            req_body = declarations[path + "/init_reqs"] if kind == "standard" else body
            reqs = re.search(r"requirements = list\((.*?)\n\t\)", req_body, re.S)[1]
            requirements = {}
            for key, value in re.findall(r'^\s*(/datum/gas/\w+|"MIN_TEMP"|"MAX_TEMP") = ([^,\n]+)', reqs, re.M):
                key = gas_by_path[key]["id"] if key in gas_by_path else key.strip('"')
                requirements[key] = evaluate(value)
            gas_reqs = [g for key in requirements for g in gases if g["id"] == key]
            major = min(gas_reqs, key=lambda g: g["rarity"])["id"]
            phase = re.search(r"priority_group = (\w+)", body)
            implementation = declarations[path + "/react"]
            rule = RULES[reaction_id]
            reactions.append(dict(id=reaction_id, path=path, kind=kind, source=source,
                                  line=read(source)[:read(source).index(implementation)].count("\n") + 1,
                                  requirements=requirements, major_gas=major,
                                  phase=evaluate(phase[1]) if phase else 0,
                                  expands_hotspot="expands_hotspot = TRUE" in body,
                                  equation=rule[0], rate=rule[1], energy=rule[2], effects=rule[3], notes=rule[4],
                                  implementation=implementation.strip()))
    assert set(RULES) == {r["id"] for r in reactions}, "Update reviewed rules for new/removed DM reactions"
    numeric = {}
    for key in expressions:
        try:
            value = evaluate(key)
            if isinstance(value, (int, float)):
                numeric[key] = value
        except (SyntaxError, ValueError, KeyError, TypeError):
            pass
    sources = [GASES, REACTIONS, ELECTROLYZER, MIXTURE, *DEFINES]
    return dict(schema=1, gases=gases, reactions=reactions, constants=numeric,
                sources={p: hashlib.sha256(read(p).encode()).hexdigest() for p in sources})


def rust(catalog):
    def number(n):
        return repr(float(n))
    gases = catalog["gases"]
    indexes = {g["id"]: i for i, g in enumerate(gases)}
    lines = ["// Generated by tools/atmos/catalog.py. Do not edit.",
             "// Preserve values derived from DM constants, including their deliberate approximations.",
             "#![allow(dead_code, clippy::excessive_precision, clippy::approx_constant)]",
             f"pub const GAS_COUNT: usize = {len(gases)};",
             f"pub const REACTION_COUNT: usize = {len(catalog['reactions'])};"]
    lines.append(f"pub const CATALOG_SIGNATURE: u32 = {signature(catalog)};")
    for i, g in enumerate(gases):
        lines.append(f'pub const {g["key"].upper()}: usize = {i};')
    lines.append("pub const HEATS: [f32; GAS_COUNT] = [" + ", ".join(number(g["specific_heat"]) for g in gases) + "];")
    for key, value in catalog["constants"].items():
        if key != "GAS_TYPE_COUNT":
            lines.append(f"pub const {key}: f32 = {number(value)};")
    lines += ["pub struct Reaction {", "    pub id: &'static str,", "    pub phase: u8,", "    pub major: usize,",
              "    pub requirements: &'static [(usize, f32)],", "    pub min_temp: f32,", "    pub max_temp: f32,",
              "    pub expands_hotspot: bool,", "}",
              "#[rustfmt::skip]", "pub const REACTIONS: &[Reaction] = &["]
    for r in catalog["reactions"]:
        req = ", ".join(f"({indexes[k]}, {number(v)})" for k, v in r["requirements"].items() if k in indexes)
        lo = number(r["requirements"].get("MIN_TEMP", 0))
        hi = number(r["requirements"]["MAX_TEMP"]) if "MAX_TEMP" in r["requirements"] else "f32::INFINITY"
        expands = str(r["expands_hotspot"]).lower()
        lines.append(f'    Reaction {{ id: "{r["id"]}", phase: {r["phase"]}, major: {indexes[r["major_gas"]]}, requirements: &[{req}], min_temp: {lo}, max_temp: {hi}, expands_hotspot: {expands} }},')
    return "\n".join(lines + ["];", ""])


def signature(data):
    return int(hashlib.sha256(json.dumps(data, sort_keys=True).encode()).hexdigest()[:6], 16)


def dm(data):
    def listing(name, paths):
        return [f"/proc/{name}()", "\tvar/static/list/types = list(",
                *[f"\t\t{path}," for path in paths], "\t)", "\treturn types", ""]
    lines = ["// Generated by tools/atmos/catalog.py. Do not edit.",
             f"#define BANDA_ATMOS_CATALOG_SIGNATURE {signature(data)}", ""]
    lines += listing("banda_atmos_gases", [g["path"] for g in data["gases"]])
    lines += listing("banda_atmos_reactions", [r["path"] for r in data["reactions"]])
    return "\n".join(lines)


def markdown(catalog):
    lines = ["# Газы и реакции BandaStation", "", "Генерируется `python tools/atmos/catalog.py`; вручную не редактировать.", "",
             "Формулы ниже описывают исполняемый DM-код. `r` — локальная скорость реакции,",
             "`T` — K, `V` — литры, количества — mol, энергия — J. Пороговые значения включительно.",
             "Если не указано иначе: Tnew=max((Told*Cold+ΔE)/Cnew,2.7) при Cnew>0.0003.", "",
             "## Газы", "", "| ID | DM type | C, J/(mol·K) | rarity |", "| --- | --- | ---: | ---: |"]
    for g in catalog["gases"]:
        lines.append(f'| `{g["id"]}` | `{g["path"]}` | {g["specific_heat"]} | {g["rarity"]} |')
    lines += ["", "## Порядок и подавление", "",
              "Четыре фазы: 1 — предварительные, 2 — образование, 3 — распад/конденсация, 4 — горение.",
              "Кандидаты собираются по major_gas (наименьшая rarity, при равенстве — первый реагент)",
              "и порядку ключей moles **до** реакций. Поэтому важны даже нулевые ключи.",
              "Температурные пороги проверяются по T в начале цикла; количества — по текущей смеси.",
              "При hypernoblium≥5 и T>20 возвращается STOP_REACTIONS, если вообще найдены кандидаты.",
              "Нельзя сортировать ключи или пересобирать кандидатов после каждого превращения.",
              "reaction_results сбрасывается только после этих ранних выходов. GC вызывается при ненулевых флагах.", "", "## Реакции"]
    for r in catalog["reactions"]:
        req = "; ".join(f"{k}≥{v:g}" if k not in ("MIN_TEMP", "MAX_TEMP") else
                        f"T{'≥' if k == 'MIN_TEMP' else '≤'}{v:g}" for k, v in r["requirements"].items())
        lines += ["", f'### `{r["id"]}`', "", f'- Источник: [{r["source"]}:{r["line"]}](../../{r["source"]}#L{r["line"]}).',
                  f'- Режим: {r["kind"]}; фаза {r["phase"]}; major_gas `{r["major_gas"]}`.',
                  f'- Вход: {req}.', f'- Превращение: {r["equation"]}.', f'- Скорость: {r["rate"]}.',
                  f'- Энергия: {r["energy"]}.', f'- Эффекты: {r["effects"]}.', f'- Особенности: {r["notes"]}']
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    catalog = extract()
    outputs = {"tools/atmos/catalog.json": json.dumps(catalog, ensure_ascii=False, indent="\t") + "\n",
               "tools/atmos/REACTIONS.md": markdown(catalog), "rust/atmos/src/catalog.rs": rust(catalog),
               "modular_bandastation/atmos_rust/code/catalog.dm": dm(catalog)}
    for path, content in outputs.items():
        target = ROOT / path
        if args.check:
            if not target.exists() or target.read_text(encoding="utf-8") != content:
                raise SystemExit(f"Stale catalog: {path}; run python tools/atmos/catalog.py")
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content, encoding="utf-8", newline="\n")
    print(f"Catalog {'checked' if args.check else 'generated'}: {len(catalog['gases'])} gases, {len(catalog['reactions'])} reactions")


if __name__ == "__main__":
    main()
