"""Compare native superconduction with complete original-game thermal passes."""
import argparse
import json
from pathlib import Path

import catalog
import network
import transport

ROOT=catalog.ROOT
REFERENCE=ROOT/"tools/atmos/thermal-world-reference.json"


def cases():
    def node(x, gas_temperature=293.15, temperature=293.15, capacity=500000, conductivity=0.4, closed=False, blocked=0):
        return dict(coordinates=[x,1,1],kind=3 if closed else 0,temperature=2.7 if closed else gas_temperature,
                    gases=[] if closed else [[1,100]],neighbors=[],terrain=dict(temperature=temperature,capacity=capacity,conductivity=conductivity,blocked=blocked))
    def case(label,nodes,conducting=None,links=()):
        for a,b in links:
            nodes[a]["neighbors"].append(b);nodes[b]["neighbors"].append(a)
        return dict(label=label,nodes=nodes,active=[i for i,n in enumerate(nodes) if n["kind"]!=3],
                    ticks=20,thermal=True,conducting=conducting or [])
    return [
        case("gas_to_wall",[node(1,1200),node(2,closed=True,capacity=100000,conductivity=0.1)],[0]),
        case("wall_to_gas",[node(1,closed=True,temperature=1500,capacity=100000,conductivity=0.2),node(2)],[0]),
        case("two_solids",[node(1,closed=True,temperature=1000,capacity=10000),node(2,closed=True,temperature=600,capacity=2000)],[0,1]),
        case("two_solids_reverse",[node(1,closed=True,temperature=1000,capacity=10000),node(2,closed=True,temperature=600,capacity=2000)],[1,0]),
        case("solid_neighbor_rearchived",[node(1,closed=True,temperature=1500,capacity=30000,conductivity=0.1),node(2,closed=True,capacity=30000,conductivity=0.1),node(3,closed=True,temperature=800,capacity=30000,conductivity=0.1)],[0,2]),
        case("separated_open_turfs",[node(1,1300),node(2)],[0]),
        case("blocked_window",[node(1,1300,blocked=4),node(2,blocked=8)],[0]),
        case("connected_open_turfs",[node(1,1300),node(2)],[0],links=[(0,1)]),
        case("cold_room",[node(1),node(2,closed=True)]),
        case("open_archive_hysteresis",[node(1,450)],[0]),
        case("radiating_solid",[node(1,closed=True,temperature=1000,capacity=100000,conductivity=0.1)],[0]),
        case("zero_conductivity_neighbor",[node(1,1200),node(2,closed=True,conductivity=0)],[0]),
        case("mixed_open_chain",[node(1,1600),node(2,450),node(3,293.15,capacity=200000,conductivity=0.03)],[0],links=[(0,1)]),
    ]


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rust",type=Path,default=ROOT/"rust/atmos/target/debug/network.exe")
    args=parser.parse_args()
    reference=json.loads(REFERENCE.read_text(encoding="utf-8"))
    inputs=cases()
    assert reference["input"]==json.loads(json.dumps(inputs)),"Different thermal inputs"
    actual=network.rust_run(inputs,args.rust)
    failures=[]
    for case,expected,observed in zip(inputs,reference["observed"],actual):
        diff=transport.difference(expected,observed)
        if diff:failures.append(dict(label=case["label"],difference=diff,expected=expected,actual=observed))
    output=ROOT/"tmp/atmos-thermal";output.mkdir(parents=True,exist_ok=True)
    (output/"failures.json").write_text(json.dumps(failures,ensure_ascii=False,indent="\t"),encoding="utf-8")
    print(f"Thermal pass: {len(inputs)-len(failures)}/{len(inputs)} match")
    for failure in failures[:8]:print(f"  {failure['label']}: {failure['difference']}")
    raise SystemExit(bool(failures))


if __name__=="__main__":main()
