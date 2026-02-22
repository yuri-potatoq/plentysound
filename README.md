# plentysound

I'm creating this project to be able to use some [Soundux](https://github.com/Soundux/Soundux) compabilities and packing into nix store since "Upstream has not had a release since 2021, nor any development activity on the main branch, and now bitrot is causing problems with this package in nixpkgs" [[ref](https://github.com/NixOS/nixpkgs/pull/283439)].


### How to test:

1. Samples test:
```bash
# list pipewire audio streams
pw-cli list-objects | grep -i source
# or 
pactl list sources short

# record the audio from desired stream. 
# Use mono channel to make it easy to load
# Keep bitrate as expteced by the application
pw-record --rate 16000Hz --channels 1 --target <source_name_or_id> ./tests/samples/output.wav
```