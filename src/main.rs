#![feature(vec_into_raw_parts)]

use anyhow::{anyhow, ensure, Context, Result};
use std::collections::HashSet;
use std::fmt::Write as FmtWrite;
use std::fs::File;
use std::io::{BufWriter, Write as IoWrite};
use std::path::PathBuf;

type Pixel = rgb::RGBA<u8>;
type ImageSet = Vec<Image>;
type Label = String;

const PIXEL_BYTES: usize = std::mem::size_of::<Pixel>();

const FILE_HEADER: &'static str = r"; ###########################################################
;              _    ____ ____  _____ _____ ____  
;             / \  / ___/ ___|| ____|_   _/ ___| 
;            / _ \ \___ \___ \|  _|   | | \___ \ 
;           / ___ \ ___) |__) | |___  | |  ___) |
;          /_/   \_\____/____/|_____| |_| |____/ 
; ###########################################################";

fn main() -> Result<()> {
    // first read all the images into a vector
    let images: ImageSet = std::env::args()
        .skip(1)
        .map(|image_file| {
            // get a handle to the file
            let file = File::open(&image_file)
                .with_context(|| format!("Failed to open {}.", &image_file))?;

            // get a reader handle to the image data
            let decoder = png::Decoder::new(file);
            let (info, mut reader) = decoder
                .read_info()
                .context("Decoder failed to read info from the image file.")?;

            // read in the first image frame
            let mut buf = vec![0; info.buffer_size()];
            reader
                .next_frame(&mut buf)
                .context("Failed to read the next frame of the PNG.")?;

            // transmute the Vec<u8> to a Vec<Pixel>
            let (ptr, len, cap) = buf.into_raw_parts();

            // assert that we aren't going to violate any memory safety guarentees
            ensure!(
                len % PIXEL_BYTES == 0,
                "Unsafe to convert vec with {} bytes to pixels.",
                len
            );
            ensure!(
                len % PIXEL_BYTES == 0,
                "Unsafe to convert vec with a capacity {} bytes to pixels.",
                cap
            );

            // do magic
            let image = unsafe {
                let pixel_ptr = ptr as *mut Pixel;
                let pixel_len = len / PIXEL_BYTES;
                let pixel_cap = cap / PIXEL_BYTES;

                Vec::from_raw_parts(pixel_ptr, pixel_len, pixel_cap)
            };

            // get the image name from the file name
            let asset_name = PathBuf::from(&image_file)
                .file_stem()
                .ok_or(anyhow!(
                    "Couldn't parse file name from path: {}",
                    &image_file
                ))?
                .to_str()
                .ok_or(anyhow!(
                    "Image path name ({}) contained invalid unicode.",
                    &image_file
                ))?
                .to_owned();

            Ok(Image::new(asset_name, image))
        })
        .collect::<Result<_>>()?;

    // without this check an empty list gives a confusing divide-by-zero error
    ensure!(!images.is_empty(), "No Images to process.");

    /* Output format
     * - Colour palette
     * - I want to be able to access images by a label (which shouldn't just be a pointer to the actual pixels.
     *   - so this implies a table from asset names to memory locations of images
     * - num pixels per byte
     * - number bits per colour
     * - the actual images
     */

    // get a handle to the file
    let mut file = BufWriter::new(
        File::create("assets.s").context("Failed to open output file - 'assets.s'")?,
    );

    // write the file header
    writeln!(file, "{}\n", FILE_HEADER)?;

    // now iterate over all the pixels and collect the unique ones.
    let palette = Palette::new_from_images(&images);
    writeln!(file, "{}", palette.to_asm()?)?;

    // calculate the number of pixels
    let bits_per_colour = (palette.len() as f64).log2().ceil() as usize;
    let pixels_per_byte = 8 / bits_per_colour;
    writeln!(file, "bits_per_colour\tEQU {}", bits_per_colour)?;
    writeln!(file, "pixels_per_byte\tEQU {}\n", pixels_per_byte)?;

    // write out the assets
    let mut labels = Vec::new();
    for image in images.into_iter() {
        let (image_label, asm) = image.to_asm(&palette, pixels_per_byte, bits_per_colour)?;
        labels.push(image_label);

        writeln!(file, "{}", asm)?;
    }

    // the address table must be aligned
    writeln!(file, "ALIGN\n")?;

    // write out the asset address table
    let aatable = "AssetAddressTable";
    let aaprefix = "_ADR";
    writeln!(file, "{}", aatable)?;
    for label in labels.iter() {
        writeln!(file, "{}{1:<28}DEFW\t{1}", aaprefix, label)?;
    }
    writeln!(file, "{}End", aatable)?;

    // write out a constant for the number of assets in the table
    writeln!(file, "\nASSET_MAX\tEQU\t({0}End - {0}) / 4\n", aatable)?;

    // write out the asset table
    for label in labels.iter() {
        writeln!(
            file,
            "ASSET{:<27}EQU\t({}{:<24} - {}) / 4",
            label, aaprefix, label, aatable
        )?;
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Image {
    name: String,
    pixels: Vec<Pixel>,
}

impl Image {
    fn new(name: String, pixels: Vec<Pixel>) -> Self {
        Self { name, pixels }
    }

    #[inline]
    fn iter(&self) -> std::slice::Iter<'_, Pixel> {
        self.pixels.iter()
    }

    fn to_asm(
        &self,
        palette: &Palette,
        pixels_per_byte: usize,
        bits_per_colour: usize,
    ) -> Result<(Label, String)> {
        let image_label: Label = format!("_{}", self.name.clone());

        let mut buf = String::new();

        // first write the label for the image
        writeln!(buf, "{}", &image_label)?;

        // now collect the pixels into bytes
        let packed: Vec<u8> = self
            .pixels
            .chunks(pixels_per_byte)
            .map(|chunk| {
                chunk.iter().rev().fold(0_u8, |acc, pixel| {
                    (acc << bits_per_colour)
                        | (palette
                            .index(pixel)
                            .expect("_Palette doesn't contain this pixel.")
                            as u8)
                })
            })
            .collect();

        // write the bytes to the buffer
        for row in packed.chunks_exact(5) {
            write!(buf, "\tDEFB 0x{:02X}", row[0])?;
            for byte in row.iter().skip(1) {
                write!(buf, ", 0x{:02X}", byte)?;
            }
            buf.push('\n');
        }

        Ok((image_label, buf))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Palette {
    colours: Vec<Pixel>,
}

impl Palette {
    fn new_from_images(images: &[Image]) -> Self {
        // iterate over all the pixels and collect the unique ones.
        let colourset: HashSet<Pixel> = images
            .iter()
            .flat_map(|image| image.iter().copied())
            .collect();

        Palette {
            colours: colourset.iter().copied().collect(),
        }
    }

    fn to_asm(&self) -> Result<String> {
        // create a buffer to write into
        let mut buf = String::new();

        // first define a label for the start of the palette
        let palette_label: Label = "Palette".into();
        writeln!(buf, "{}", palette_label)?;

        // now write out the colours
        for colour in self.colours.iter() {
            writeln!(
                buf,
                "\tDEFB 0x{:02X}, 0x{:02X}, 0x{:02X}, 0x{:02X}",
                colour.r, colour.g, colour.b, colour.a
            )?;
        }

        Ok(buf)
    }

    fn index(&self, colour: &Pixel) -> Option<usize> {
        self.colours.iter().position(|c| c == colour)
    }

    #[inline]
    fn len(&self) -> usize {
        self.colours.len()
    }
}
