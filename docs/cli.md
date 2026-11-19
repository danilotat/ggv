# The command line

```sh
ggv info sample.bam
ggv query pileup  sample.bam chr7:55,191,822 -f ref.fa --format json
ggv query pileup  sample.bam chr7:55191000-55192000 --min-alt-fraction 0.2
ggv query summary sample.bam chr7:55191000-55192000 -f ref.fa --format json
```

A single base always gets a row from `query pileup`, at depth 0 when no read covers it; a
region lists covered positions only unless `-a` is given. In `query summary`, a run of bases
deleted in enough reads is one variable site with its `length`, not one per base; a base in it
where the other reads carry a commoner mismatch is listed as well, without cutting the run.

A file without an index gets one from `ggv index` (BAM `.bai`, or `.csi` when a contig is too
long for one; CRAM `.crai`; bgzipped VCF `.tbi`; FASTA `.fai`, and `.gzi` when bgzipped), written
next to the file as samtools and tabix would:

```sh
ggv index sample.bam calls.vcf.gz ref.fa     # --force builds one again
```

## Files at a URL (S3, GCS, HTTPS)

BAM, CRAM, FASTA, 2bit and bigWig files can be read straight from an `https://`, `http://`,
`s3://` or `gs://` address, as IGV does: GGV fetches only the byte ranges a query needs, never
the whole file. The 1000 Genomes CRAMs on S3 work as given, with the reference they were aligned
to (also on S3, FASTA with its `.fai`):

```sh
ggv query pileup \
  https://s3.amazonaws.com/1000genomes/data/NA21144/alignment/NA21144.alt_bwamem_GRCh38DH.20150718.GIH.low_coverage.cram \
  chr20:1,000,000-1,000,100 \
  -f https://s3.amazonaws.com/1000genomes/technical/reference/GRCh38_reference_genome/GRCh38_full_analysis_set_plus_decoy_hla.fa
ggv info s3://1000genomes/data/NA21144/alignment/NA21144.alt_bwamem_GRCh38DH.20150718.GIH.low_coverage.cram
```

- The index is looked for where it would be next to a local file (`x.bam.bai`, `x.bam.csi`,
  `x.bai`; `x.cram.crai`, `x.crai`; for a FASTA `x.fa.fai`, plus `x.fa.gz.gzi` when bgzipped).
  When it lives somewhere else, as with a presigned URL, give it with `--index` (repeat it
  for several alignment files, in the same order). `ggv index` can't write next to a remote file.
- `s3://bucket/key` reads the bucket's public HTTPS address. `AWS_REGION` picks the regional
  endpoint, and `AWS_ENDPOINT_URL` points at an S3-compatible server. `gs://bucket/key` reads
  `https://storage.googleapis.com/bucket/key`. Presigned URLs work as they are; their query string
  (the signature) is left out of error messages. Private buckets that need signed requests
  aren't supported yet.
- Indexes are fetched whole once and kept in the cache folder (`~/.cache/ggv/remote` on Linux),
  keyed by URL and ETag, so opening the same file again doesn't fetch them again. Data is cached
  in memory only, in 256 KB blocks, up to `GGV_REMOTE_CACHE_MB` (default 64) per file. A file
  that changes on the server while it is open is reported as an error, never read as a mix of
  the two versions.
- Gene annotations (GTF, GFF3, BED) and bedGraph/SEG text at a URL are downloaded whole, since
  they have no index. VCF/BCF can't be read from a URL yet.

```sh
ggv render sample.bam chr7:55,191,822 -f ref.fa -o snv.png            # ±50 bp, base outlined
ggv render sample.bam chr7:55191000-55241000 -f ref.fa -o view.svg    # SVG, real text
ggv render sample.bam chr7:55,191,822 -f ref.fa --scale 2 --color-by strand -o snv@2x.png
ggv render hifi.bam chr20:58888000-58890000 -f ref.fa --color-by base-mods -o methylation.png
```

`--color-by` takes `strand`, `haplotype` (HP tag), `insert-size` (pair orientation and insert
size) or `base-mods` (5mC and other MM/ML calls, red likely modified, blue not), with the app's
colours. `--group` puts reads in labelled bands as the app does: `strand`, `haplotype`,
`read-group`, `alleles` (phased from the reads' own alleles) or, for a single base, `base`;
`--sort` orders them by what they show at a single base. `--pairs` puts each pair's mates on
one row joined across the insert (IGV's "view as pairs"), and `--link` each split read's parts.
`--color-by base-mods --group haplotype` shows allele-specific methylation (imprinting) at a
glance. `ggv batch` takes `--color-by`, `--group`, `--pairs` and `--link` too, and MCP `render`
takes `color_by`, `group_by`, `pairs` and `link` (attached, it uses the window's colouring,
grouping, pairs and linking).

Renders follow IGV's conventions: coverage with allele-coloured bars where ≥20% of reads
disagree with the reference, reads packed into rows (squished past 40 rows), mismatches
coloured A/C/G/T and shaded by base quality, letters when zoomed in, black lines for deletions
(labelled from 2 bp), purple bars for insertions (≥5 bp when zoomed out), and strand arrows.
Regions wider than `--read-window` (1 Mb; the app's own default is 50 kb) show coverage only.

**Batch snapshots**: a picture and an evidence report for every place in a list, with no
window, on all cores:

```sh
ggv batch sites.bed -b tumor.bam -b normal.bam -f ref.fa -v calls.vcf.gz -g genes.gtf.gz -o review/
```

The list is BED, VCF, or one locus per line (`chr1:100`, `chr1:100-200`, text after it is a
label), gzipped or not. For each place GGV writes `NNN-contig-pos.png` (every BAM stacked, the
place outlined; single bases get ±50 bp) and `NNN-contig-pos.html` (the image embedded, the VCF
records there, the consequences of each change, allele counts by strand at a single base or a
region summary with its variable sites). `index.html` is a contact sheet of every place linking
to its report, and `summary.tsv` has one row per place (consequence, depth and top allele per
BAM). A place that fails (an unknown contig) is reported and skipped. Nine places over two
BAMs take about 0.3 s. The exit status is 0 when every place was written, 3 when some failed
(`summary.tsv` says which and why) and 1 when none was.

Each position reports depth, A/C/G/T/N by strand, deletions, insertions after the base,
reference skips, mean MAPQ and MAPQ-0 count, mean base quality, soft clips starting/ending
there, and the top alt allele with its fraction. `summary` adds read-filter counts, mean/median
depth, breadth at 1/10/20/30×, and the most variable sites.

Defaults count what IGV shows: unmapped, secondary, QC-fail and duplicate reads are dropped
(`--exclude-flags 0x704`), no MAPQ or base-quality floor, overlapping mates both count.

Without a reference (`-f` or `--genome`), `alt_fraction`, `top_alt` and a summary's variable
sites compare each position with its most common allele, so a homozygous variant is not
reported as one. `query pileup` and `query summary` say so on stderr.
