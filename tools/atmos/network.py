"""Compare connected-tile Rust passes with extracted Banda process_cell and excited-group procedures."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import random

import catalog
import compare
import transport

ROOT = catalog.ROOT
WORK = ROOT / "tmp/atmos-network"
REFERENCE = ROOT / "tools/atmos/network-reference.json"
IMMUTABLE = "code/modules/atmospherics/gasmixtures/immutable_mixtures.dm"
TURFS = "code/modules/atmospherics/environmental/LINDA_turf_tile.dm"
AIR = "code/controllers/subsystem/air.dm"


def cases():
    def node(position, gases, temperature=293.15):
        return dict(coordinates=position, gases=list(gases.items()), temperature=temperature, neighbors=[])

    def case(label, nodes, active=None, ticks=20, blocked=False):
        if not blocked:
            for i, cell in enumerate(nodes):
                cell["neighbors"] = [j for j, other in enumerate(nodes) if sum(abs(a-b) for a,b in zip(cell["coordinates"],other["coordinates"])) == 1]
        return dict(label=label,nodes=nodes,active=list(range(len(nodes))) if active is None else active,ticks=ticks)

    fixed = [
        case("equal_air_sleeps",[node([1,1,1],{0:20,1:80}),node([2,1,1],{0:20,1:80})]),
        case("two_closed_rooms",[node([1,1,1],{0:100,1:300}),node([2,1,1],{})]),
        case("wave_from_one_active_cell",[node([x,1,1],{1:400 if x==1 else 0}) for x in range(1,4)],active=[0]),
        case("neighbor_is_rearchived",[node([2,1,1],{1:100}),node([1,1,1],{1:300}),node([3,1,1],{})],active=[1,2]),
        case("vertical_opening",[node([1,1,1],{1:400}),node([1,1,2],{})]),
        case("closed_door",[node([1,1,1],{1:400}),node([2,1,1],{})],blocked=True),
        case("groups_merge",[node([x,1,1],{1:amount}) for x,amount in enumerate((400,0,200,0),1)]),
        case("hot_and_cold",[node([1,1,1],{0:100,1:300},2000),node([2,1,1],{0:50,1:150},200)]),
        case("six_cardinal_neighbors",[node(p,{1:float(i*50)}) for i,p in enumerate(([3,3,2],[2,3,2],[4,3,2],[3,2,2],[3,4,2],[3,3,1],[3,3,3]))]),
        case("all_gases_suppressed",[node([1,1,1],{i:i+1 for i in range(20)},300),node([2,1,1],{i:20-i for i in reversed(range(20))},500)]),
        case("nitrous_decomposition_after_share",[node([1,1,1],{6:20},2000),node([2,1,1],{1:100},300)]),
        case("condensation_keeps_group_volatile",[node([1,1,1],{4:1},190),node([2,1,1],{1:50,4:1},190)]),
    ]
    rng=random.Random(220)
    generated=[
        case("groups_from_opposite_ends",[node([x,1,1],{1:amount}) for x,amount in enumerate((400,0,200,0),1)],active=[0,3]),
        case("persistent_condensation",[node([1,1,1],{4:10},190),node([2,1,1],{1:50,4:10},190)]),
    ]
    for index in range(24):
        nodes=[node([x,y,1],{0:rng.uniform(0,100),1:rng.uniform(0,400)},rng.uniform(200,600)) for x in range(1,4) for y in range(1,3)]
        active=rng.sample(range(len(nodes)),rng.randrange(1,len(nodes)+1))
        generated.append(case(f"random:{index}",nodes,active=active))
    return fixed,generated


def boundary_cases():
    def node(x, gases=None, temperature=293.15, kind=0, planetary=None, z=1):
        result=dict(coordinates=[x,1,z],gases=list((gases or {}).items()),temperature=temperature,neighbors=[],kind=kind)
        if planetary: result["planetary"]=dict(temperature=planetary[0],gases=list(planetary[1].items()))
        return result
    def space(x,z=1): return node(x,temperature=2.7,kind=1,z=z)
    def case(label,nodes,active=None):
        for i,cell in enumerate(nodes):
            cell["neighbors"]=[j for j,other in enumerate(nodes) if sum(abs(a-b) for a,b in zip(cell["coordinates"],other["coordinates"]))==1]
        return dict(label=label,nodes=nodes,active=list(range(len(nodes))) if active is None else active,ticks=20)
    earth=(293.15,{0:20,1:80})
    fixed=[
        case("breach_space_runs_first",[node(1,{0:100,1:300},800),space(2)]),
        case("breach_floor_runs_first",[node(1,{0:100,1:300},800),space(2)],active=[1,0]),
        case("space_activated_by_floor",[node(1,{1:400}),space(2)],active=[0]),
        case("two_rooms_share_one_space",[node(1,{0:100,1:300}),space(2),node(3,{1:50},2000)]),
        case("two_space_boundaries",[space(1),node(2,{0:100,1:300}),space(3)]),
        case("shared_space_datum",[node(1,{0:100}),space(2),node(5,{1:300},1000),space(6)]),
        case("vertical_breach",[node(1,{1:400}),space(1,z=2)]),
        case("planetary_equal_air",[node(1,{0:20,1:80},planetary=earth)]),
        case("planetary_vacuum",[node(1,temperature=2.7,planetary=earth)]),
        case("planetary_hot_gas",[node(1,{3:100},2000,planetary=earth)]),
        case("planetary_and_space",[node(1,{0:100,1:300},800,planetary=earth),space(2)]),
        case("planetary_group_breakdown",[node(1,{0:1,1:1},500,planetary=earth),node(2,{1:500},700,planetary=earth)]),
        case("immutable_planetary_sharer",[node(1,{0:100},2000),node(2,{1:100},300,kind=2)]),
        case("immutable_planetary_caller",[node(1,{0:100},2000),node(2,{1:100},300,kind=2)],active=[0,1]),
    ]
    rng=random.Random(516220)
    generated=[]
    for i in range(32):
        nodes=[node(1,{0:rng.uniform(0,500),1:rng.uniform(0,500)},rng.uniform(200,2000)),space(2),
               node(3,{0:rng.uniform(0,500),1:rng.uniform(0,500)},rng.uniform(200,2000))]
        if i%2: nodes[0]["planetary"]=dict(temperature=earth[0],gases=list(earth[1].items()))
        generated.append(case(f"boundary_random:{i}",nodes,active=rng.sample(range(3),3)))
    return fixed,generated


def harness(data):
    transport.WORK=WORK
    transport.harness(data)
    compare.write("network-support.dm",catalog.read("tools/atmos/transport.dm.in").split("/world/New()",1)[0])
    text=catalog.read(TURFS)
    start=text.index("#ifdef TRACK_MAX_SHARE\n#define LAST_SHARE_CHECK")
    compare.write("cell-macros.dm",text[start:text.index("/turf/proc/process_cell(",start)])
    blocks=catalog.blocks(text)
    paths=["/turf/open/process_cell","/turf/open/proc/consider_pressure_difference","/datum/excited_group",
           "/datum/excited_group/New",*["/datum/excited_group/proc/"+p for p in ("add_turf","merge_groups","reset_cooldowns","self_breakdown","dismantle","garbage_collect")]]
    compare.write("cells-impl.dm","\n".join(blocks[p] for p in paths))
    air=catalog.blocks(catalog.read(AIR))
    names=("process_active_turfs","process_excited_groups","remove_from_active","sleep_active_turf","add_to_active")
    compare.write("air-impl.dm","\n".join(air["/datum/controller/subsystem/air/proc/"+p].replace("/datum/controller/subsystem/air/proc/","/datum/oracle_air/proc/") for p in names))
    mixture=catalog.blocks(catalog.read(catalog.MIXTURE))
    compare.write("more-mixture.dm","\n".join(mixture["/datum/gas_mixture/proc/"+p] for p in ("compare","copy_from")))
    compare.write("network.dm",catalog.read("tools/atmos/network.dm.in"))
    immutable=catalog.blocks(catalog.read(IMMUTABLE))
    unused={"merge","copy","copy_from_ratio","remove","remove_ratio","parse_string_immutable"}
    compare.write("immutable.dm","\n".join(body for path,body in immutable.items() if path.rsplit("/",1)[-1] not in unused))
    compare.write("reaction-types.dm","/proc/network_reactions()\n\treturn list("+",".join(r["path"] for r in data["reactions"])+")\n")
    files=("definitions.dm","conduction.dm","oracle-header.dm","setup.dm","mixture.dm","transport-impl.dm","more-mixture.dm",
           "reactions.dm","electrolyzer.dm","cell-macros.dm","network-support.dm","immutable.dm","network.dm","reaction-types.dm","cells-impl.dm","air-impl.dm")
    compare.write("network.dme","\n".join(f'#include "{p}"' for p in files)+"\n")


def source_run(byond, inputs):
    compare.write("cases.json",json.dumps(inputs))
    print(compare.command([byond/("dm.exe" if os.name=="nt" else "DreamMaker"),"network.dme"], cwd=WORK))
    result=WORK/"dm-output.json"
    if result.exists(): result.unlink()
    compare.write("network.log","")
    log=compare.command([byond/("dd.exe" if os.name=="nt" else "DreamDaemon"),"network.dmb","-safe","-close","-invisible","-logself"], cwd=WORK)
    log+=(WORK/"network.log").read_text(encoding="utf-8-sig",errors="replace")
    if "runtime error:" in log.lower() or "ORACLE_FAILURE" in log: raise RuntimeError(log[:8000])
    output=json.loads(result.read_text(encoding="utf-8-sig"))
    assert len(output["cases"])==len(inputs),"Incomplete DM run"
    return output


def rust_run(inputs, rust):
    lines=[]
    for case in inputs:
        words=[case["ticks"],len(case["nodes"]),int(bool(case.get("hotspots"))) | (2 if case.get("thermal") else 0)]
        for n in case["nodes"]:
            words+=n["coordinates"]+[n.get("kind",0),n["temperature"],len(n["gases"])]
            words+=[v for pair in n["gases"] for v in pair]
            planet=n.get("planetary")
            words+=[int(bool(planet))]
            if planet:
                words += [planet["temperature"],len(planet["gases"])] + [v for pair in planet["gases"] for v in pair]
            terrain=n.get("terrain")
            words += [int(bool(terrain))]
            if terrain: words += [terrain[k] for k in ("temperature","capacity","conductivity","blocked")]
            words+=[len(n["neighbors"]),*n["neighbors"]]
        words+=[len(case["active"]),*case["active"]]
        conducting=case.get("conducting",[])
        words += [len(conducting),*conducting]
        lines.append(" ".join(map(str,words)))
    actual=[json.loads(line) for line in compare.command([rust],input_text="\n".join(lines)+"\n").splitlines()]
    assert len(actual)==len(inputs),"Incomplete Rust run"
    return actual


def main():
    global WORK
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--byond",type=Path,required=True)
    parser.add_argument("--source-root",type=Path,help="Untouched DM source tree for the original solver oracle")
    parser.add_argument("--rust",type=Path,default=ROOT/"rust/atmos/target/debug/network.exe")
    parser.add_argument("--record",action="store_true")
    parser.add_argument("--boundaries",action="store_true")
    parser.add_argument("--world-reference",type=Path)
    args=parser.parse_args()
    if args.source_root:
        catalog.SOURCE_ROOT = args.source_root.resolve()
    if args.boundaries: WORK = ROOT / "tmp/atmos-boundary"
    reference = ROOT / "tools/atmos/boundary-reference.json" if args.boundaries else REFERENCE
    if args.record and reference.exists(): parser.error("Reference already exists; do not overwrite silently")
    data=catalog.extract();harness(data)
    fixed,generated=boundary_cases() if args.boundaries else cases();inputs=fixed+generated
    expected=source_run(args.byond,inputs)
    sources={p:hashlib.sha256(catalog.read(p).encode()).hexdigest() for p in [*data["sources"],TURFS,AIR,"code/__DEFINES/atmospherics/atmos_helpers.dm"]}
    if args.boundaries: sources[IMMUTABLE]=hashlib.sha256(catalog.read(IMMUTABLE).encode()).hexdigest()
    snapshot=dict(schema=1,byond=expected["version"],source_sha256=sources,cases=[dict(input=i,expected=o) for i,o in zip(fixed,expected["cases"])])
    snapshot=json.loads(json.dumps(snapshot))
    if args.record:
        reference.write_text(json.dumps(snapshot,ensure_ascii=False,indent="\t")+"\n",encoding="utf-8")
    else:
        saved=json.loads(reference.read_text(encoding="utf-8"))
        if args.source_root:
            # Adapter hooks can change whole-file hashes; the untouched solver must reproduce every saved state exactly.
            assert saved["schema"]==snapshot["schema"] and saved["byond"]==snapshot["byond"] and saved["cases"]==snapshot["cases"], "Original DM solver differs from the captured baseline"
            compare.write("source-validation.json", json.dumps(dict(source_root=str(catalog.SOURCE_ROOT), source_sha256=sources, captured_source_sha256=saved["source_sha256"], captured_states_exact=True), indent=2))
        else:
            assert saved==snapshot,"DM graph behavior/source/version changed; review the baseline explicitly"
    actual=rust_run(inputs,args.rust)
    if args.world_reference:
        if not args.boundaries: parser.error("World reference is supported for the boundary suite")
        world=json.loads(args.world_reference.read_text(encoding="utf-8"))
        assert world["input"]==json.loads(json.dumps(fixed)),"Different world inputs"
        assert world["byond"]==expected["version"],"Different world BYOND"
        changed = {p for p,h in world["source_sha256"].items() if hashlib.sha256(catalog.read(p).encode()).hexdigest()!=h}
        if args.source_root:
            assert changed <= {TURFS, AIR}, "Original world physics source changed outside adapter hook files"
        else:
            assert not changed,"World source changed"
        expected["cases"][:len(fixed)]=world["observed"]
    failures=[];pressure=[]
    for case,e,a in zip(inputs,expected["cases"],actual):
        e=copy.deepcopy(e);a=copy.deepcopy(a)
        for tick,(et,at) in enumerate(zip(e,a)):
            for node,(ec,ac) in enumerate(zip(et["cells"],at["cells"])):
                diff=transport.difference(ec.pop("pressure"),ac.pop("pressure"))
                if diff: pressure.append(dict(label=case["label"],tick=tick,node=node,difference=diff))
        diff=transport.difference(e,a)
        if diff: failures.append(dict(label=case["label"],difference=diff,input=case,expected=e,actual=a))
    compare.write("failures.json",json.dumps(failures,ensure_ascii=False,indent="\t"))
    compare.write("pressure-deviations.json",json.dumps(pressure,ensure_ascii=False,indent="\t"))
    print(f"Network state: {len(inputs)-len(failures)}/{len(inputs)} match; {len(pressure)} separate pressure deviations")
    for failure in failures[:8]: print(f"  {failure['label']}: {failure['difference']}")
    raise SystemExit(bool(failures))


if __name__=="__main__": main()
