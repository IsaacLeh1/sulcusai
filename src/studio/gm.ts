// SPDX-License-Identifier: AGPL-3.0-only
// The 128 General MIDI instruments a MIDI track can play (the sound set
// Compose uses has them all), and drums.
import { tx } from "../i18n";

export const GM: string[] = [
  "Acoustic grand piano", "Bright piano", "Electric grand piano", "Honky-tonk piano", "Electric piano 1", "Electric piano 2", "Harpsichord", "Clavinet",
  "Celesta", "Glockenspiel", "Music box", "Vibraphone", "Marimba", "Xylophone", "Tubular bells", "Dulcimer",
  "Drawbar organ", "Percussive organ", "Rock organ", "Church organ", "Reed organ", "Accordion", "Harmonica", "Tango accordion",
  "Nylon guitar", "Steel guitar", "Jazz guitar", "Clean electric guitar", "Muted guitar", "Overdriven guitar", "Distortion guitar", "Guitar harmonics",
  "Acoustic bass", "Fingered bass", "Picked bass", "Fretless bass", "Slap bass 1", "Slap bass 2", "Synth bass 1", "Synth bass 2",
  "Violin", "Viola", "Cello", "Contrabass", "Tremolo strings", "Pizzicato strings", "Harp", "Timpani",
  "String ensemble 1", "String ensemble 2", "Synth strings 1", "Synth strings 2", "Choir aahs", "Voice oohs", "Synth voice", "Orchestra hit",
  "Trumpet", "Trombone", "Tuba", "Muted trumpet", "French horn", "Brass section", "Synth brass 1", "Synth brass 2",
  "Soprano sax", "Alto sax", "Tenor sax", "Baritone sax", "Oboe", "English horn", "Bassoon", "Clarinet",
  "Piccolo", "Flute", "Recorder", "Pan flute", "Blown bottle", "Shakuhachi", "Whistle", "Ocarina",
  "Square lead", "Saw lead", "Calliope lead", "Chiff lead", "Charang lead", "Voice lead", "Fifths lead", "Bass and lead",
  "New age pad", "Warm pad", "Polysynth pad", "Choir pad", "Bowed pad", "Metallic pad", "Halo pad", "Sweep pad",
  "Rain", "Soundtrack", "Crystal", "Atmosphere", "Brightness", "Goblins", "Echoes", "Sci-fi",
  "Sitar", "Banjo", "Shamisen", "Koto", "Kalimba", "Bagpipe", "Fiddle", "Shanai",
  "Tinkle bell", "Agogo", "Steel drums", "Woodblock", "Taiko drum", "Melodic tom", "Synth drum", "Reverse cymbal",
  "Guitar fret noise", "Breath noise", "Seashore", "Bird tweet", "Telephone ring", "Helicopter", "Applause", "Gunshot",
].map((n) => tx(n));

export const DRUM_KIT = tx("Drums");

/** A good first instrument for a track from its name ("Bass" plays a bass). */
export function programFor(name: string): number {
  const n = name.toLowerCase();
  if (/drum|perc|beat|kick|snare|hat/.test(n)) return 128;
  if (/bass/.test(n)) return 33;
  if (/guitar/.test(n)) return 25;
  if (/string|violin|cello/.test(n)) return 48;
  if (/organ/.test(n)) return 16;
  if (/brass|horn|trumpet/.test(n)) return 61;
  if (/sax/.test(n)) return 65;
  if (/flute/.test(n)) return 73;
  if (/synth|lead/.test(n)) return 81;
  if (/pad/.test(n)) return 89;
  if (/vocal|voice|choir/.test(n)) return 52;
  return 0;
}
