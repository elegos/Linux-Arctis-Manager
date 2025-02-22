from dataclasses import dataclass
import os


@dataclass
class PactlDevice:
    name: str
    description: str


def pactl_get_available_sinks() -> list[PactlDevice]:
    raw_result = os.popen('''
    pactl list sinks | awk '
        /^Sink/ { if (name && desc) { print name, ":", desc }; name=""; desc="" }
        /node.name = / {name=$3}
        /device.description = / {desc=$3; for(i=4;i<=NF;i++) desc=desc" "$i}
        END { if (name && desc) { print name, ":", desc }}'
    ''').readlines()

    return [PactlDevice(name.strip(' "\n'), description.strip(' "\n')) for name, description in [line.split(':') for line in raw_result]]
