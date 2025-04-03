# non-spatial-input
Tools you can easily snap together to get non-spatial input into stardust, such as keyboard/mouse input.

> [!IMPORTANT]  
> Requires the [Stardust XR Server](https://github.com/StardustXR/server) to be running. For launching 2D applications, [Flatland](https://github.com/StardustXR/flatland) also needs to be running.  

If you installed the Stardust XR server via:  
```note
sudo dnf group install stardust-xr
```
Or if you installed via the [installation script](https://github.com/cyberneticmelon/usefulscripts/blob/main/stardustxr_setup.sh), non-spatial-input comes pre-installed

## How to Use
### Input Methods
`Manifold` opens up a window on your desktop that when made active will pipe keyboard and (eventually) mouse pointer information into either Azimuth or Simular.

`Eclipse` is a [libinput](https://wayland.freedesktop.org/libinput/doc/latest/) client that can also be piped into Azimuth or Simular. The most common use case for libinput would be running Stardust XR in a headless environment, i.e. integrated into a standalone headset.

### Output Methods
`Azimuth` creates a virtual pointer in 3D space (Currently broken)

`Simular` directs the keyboard and mouse input to whatever window you are currently looking at.

Use these by piping them in: 
```bash
mainfold | simular // Most common use case
eclipse | simular
eclipse | azimuth
manifold | azimuth
```

## Manual Installation
Clone the repository and after the server is running:
```sh
cargo run -args
```
