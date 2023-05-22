# image2arm
A compiler from a set of sprites to ARM assembly DEFB's where the sprites can share a global colour table.

## Example Usage
`cargo run --release -- <sprites>`

This will create a fill called `assets.s` which contains the colour table, the sprites and some defines to access sprites by their index.
