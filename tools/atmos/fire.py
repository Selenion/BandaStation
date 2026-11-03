"""Compare complete native hotspot passes with captured real Banda cell/hotspot/group passes."""
import argparse
import json
from pathlib import Path

import catalog
import network
import transport

ROOT=catalog.ROOT
REFERENCE=ROOT/"tools/atmos/fire-world-reference.json"


def cases():
    def node(position,gases,temperature):
        return dict(coordinates=position,gases=list(gases.items()),temperature=temperature,kind=0,neighbors=[])
    def case(label,nodes,active=None):
        for i,cell in enumerate(nodes):
            cell["neighbors"]=[j for j,other in enumerate(nodes) if sum(abs(a-b) for a,b in zip(cell["coordinates"],other["coordinates"]))==1]
        return dict(label=label,nodes=nodes,active=list(range(len(nodes))) if active is None else active,ticks=20,hotspots=True)
    result=[]
    for label,temperature,gases in (
        ("plasma",1000,{0:100,3:10}),("hydrogen",700,{0:100,13:10}),
        ("tritium",8000,{0:100,8:10}),("freon",100,{0:100,12:10}),
        ("freon_280",280,{0:100,12:10}),("mixed_fuels",1644,{0:100,3:10,13:10}),
        ("oxygen_just_above_birth",1644,{0:0.51,3:1}),
        ("nitrous_feeds_fire",2000,{6:20,3:10}),
        ("halon_suppresses_fire",1000,{0:100,3:10,17:1}),
    ):
        result.append(case("hotspot:"+label,[node([1,1,1],gases,temperature)]))
    for label,temperature,fuel in (("hot",2000,3),("cold",100,12)):
        result.append(case("spread:"+label,[node([x,1,1],{0:100,fuel:10},temperature if x==1 else 293.15) for x in range(1,4)]))
        result.append(case("spread:"+label+"_vertical",[node([1,1,z],{0:100,fuel:10},temperature if z==1 else 293.15) for z in (1,2)]))
    result.append(case("fire_to_inert_neighbors",[node([1,1,1],{0:100,3:10},2000),node([2,1,1],{1:100},300)]))
    return result


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust",type=Path,default=ROOT/"rust/atmos/target/debug/network.exe")
    args=parser.parse_args()
    reference=json.loads(REFERENCE.read_text(encoding="utf-8"))
    inputs=cases()
    assert json.loads(json.dumps(inputs))==reference["input"],"Different fire reference inputs"
    actual=network.rust_run(inputs,args.rust)
    failures=[]
    for case,expected,observed in zip(inputs,reference["observed"],actual):
        diff=transport.difference(expected,observed)
        if diff: failures.append(dict(label=case["label"],difference=diff,expected=expected,actual=observed))
    output=ROOT/"tmp/atmos-fire"
    output.mkdir(parents=True,exist_ok=True)
    (output/"failures.json").write_text(json.dumps(failures,ensure_ascii=False,indent="\t"),encoding="utf-8")
    print(f"Full fire pass: {len(inputs)-len(failures)}/{len(inputs)} match")
    for failure in failures[:8]: print(f"  {failure['label']}: {failure['difference']}")
    raise SystemExit(bool(failures))


if __name__=="__main__":main()
