use vids_core::{
    executor::expected_streams,
    parse::{ExpectedStream, ProgressAggregator, parse_line},
    types::{DownloadStatus, ErrorCategory, OutputLine, OutputStream, ParsedLine},
};

const DOWNLOAD_STDOUT: &str = include_str!("fixtures/ytdlp/download_bbb_240p.stdout");
const DOWNLOAD_STDERR: &str = include_str!("fixtures/ytdlp/download_bbb_240p.stderr");
const NICONICO_SM9_STDOUT: &str = include_str!("fixtures/ytdlp/download_niconico_sm9.stdout");
const NICONICO_SM9_PROBE: &str = include_str!("fixtures/ytdlp/probe_niconico_sm9_formats.json");

fn parsed_lines(fixture: &str, stream: OutputStream) -> Vec<ParsedLine> {
    fixture
        .lines()
        .map(|text| {
            parse_line(&OutputLine {
                stream,
                text: text.to_owned(),
            })
        })
        .collect()
}

#[test]
fn download_fixture_lines_map_to_expected_variants() {
    let stdout = parsed_lines(DOWNLOAD_STDOUT, OutputStream::Stdout);
    let stderr = parsed_lines(DOWNLOAD_STDERR, OutputStream::Stderr);
    assert_eq!(
        stdout
            .iter()
            .filter(|line| matches!(line, ParsedLine::Download(_)))
            .count(),
        57
    );
    assert_eq!(
        stdout
            .iter()
            .filter(|line| matches!(line, ParsedLine::FinalPath(_)))
            .count(),
        1
    );
    assert_eq!(
        stderr
            .iter()
            .filter(|line| matches!(line, ParsedLine::Postprocess(_)))
            .count(),
        4
    );
    assert_eq!(
        stderr
            .iter()
            .filter(|line| matches!(line, ParsedLine::Warning(_)))
            .count(),
        1
    );
    assert!(
        stdout
            .iter()
            .all(|line| !matches!(line, ParsedLine::Other(_)))
    );
    assert!(
        stderr
            .iter()
            .all(|line| !matches!(line, ParsedLine::Other(_)))
    );
    let final_path = stdout.iter().find_map(|line| match line {
        ParsedLine::FinalPath(path) => Some(path),
        _ => None,
    });
    assert!(
        final_path
            .expect("fixture should contain a final path")
            .ends_with(r"Big Buck Bunny [YE7VzlLtp-4].mp4")
    );
}

#[test]
fn error_fixtures_map_to_expected_categories() {
    let cases = [
        (
            include_str!("fixtures/ytdlp/error_unsupported_url.stderr"),
            ErrorCategory::UnsupportedUrl,
        ),
        (
            include_str!("fixtures/ytdlp/error_niconico_members_only.stderr"),
            ErrorCategory::LoginRequired,
        ),
        (
            include_str!("fixtures/ytdlp/error_unavailable_youtube.stderr"),
            ErrorCategory::Unavailable,
        ),
        (
            include_str!("fixtures/ytdlp/error_youtube_not_found.stderr"),
            ErrorCategory::Unavailable,
        ),
    ];
    for (fixture, expected) in cases {
        let errors: Vec<_> = parsed_lines(fixture, OutputStream::Stderr)
            .into_iter()
            .filter_map(|line| match line {
                ParsedLine::Error { category, .. } => Some(category),
                _ => None,
            })
            .collect();
        assert_eq!(errors, vec![expected]);
    }
}

#[test]
fn progress_fixture_is_monotonic_with_known_and_unknown_sizes() {
    let progress: Vec<_> = parsed_lines(DOWNLOAD_STDOUT, OutputStream::Stdout)
        .into_iter()
        .filter_map(|line| match line {
            ParsedLine::Download(progress) => Some(progress),
            _ => None,
        })
        .collect();
    for sizes in [[Some(6_540_562), Some(28_921_366)], [None, None]] {
        let mut aggregator = ProgressAggregator::new(vec![
            ExpectedStream {
                format_id: "395".to_owned(),
                size: sizes[0],
                approx_size: sizes[0],
            },
            ExpectedStream {
                format_id: "258".to_owned(),
                size: sizes[1],
                approx_size: sizes[1],
            },
        ]);
        let mut previous = 0.0;
        let mut final_fraction = None;
        for update in &progress {
            let job = aggregator.update(update);
            if let Some(fraction) = job.fraction {
                assert!(fraction >= previous);
                previous = fraction;
                final_fraction = Some(fraction);
            }
        }
        assert_eq!(final_fraction, Some(1.0));
    }
}

#[test]
fn niconico_progress_uses_probe_bitrate_weights_and_fragments() {
    let probe = serde_json::from_str(NICONICO_SM9_PROBE).expect("probe fixture should be JSON");
    let expected = expected_streams(&probe);
    assert_eq!(expected.len(), 2);
    assert_eq!(expected[0].size, None);
    assert_eq!(expected[0].approx_size, Some(13_158_280));
    assert_eq!(expected[1].size, None);
    assert_eq!(expected[1].approx_size, Some(4_189_880));

    let progress = download_progress(NICONICO_SM9_STDOUT);
    let mut aggregator = ProgressAggregator::new(expected);
    let mut previous = 0.0;
    let mut first_audio = None;
    let last_audio_downloading = progress
        .iter()
        .rposition(|update| {
            update.status == DownloadStatus::Downloading
                && update.filename.contains("faudio-aac-128kbps")
                && update.fragment_index <= update.fragment_count
        })
        .expect("fixture should contain an audio last-fragment update");

    for (index, update) in progress.iter().enumerate() {
        let fraction = aggregator
            .update(update)
            .fraction
            .expect("probe weights should provide a fraction on every line");
        assert!(fraction >= previous, "fraction decreased at line {index}");
        previous = fraction;
        if update.filename.contains("faudio-aac-128kbps") {
            first_audio.get_or_insert(fraction);
            if update.status == DownloadStatus::Downloading && index < last_audio_downloading {
                assert!(
                    fraction < 1.0,
                    "fraction completed before the last audio fragment"
                );
            }
        }
    }

    let first_audio = first_audio.expect("fixture should contain audio updates");
    assert!((0.70..0.80).contains(&first_audio));
    assert_eq!(previous, 1.0);
}

#[test]
fn niconico_progress_stays_unknown_without_stream_weights() {
    let expected = ["video-h264-360p-lowest-1", "audio-aac-128kbps"]
        .into_iter()
        .map(|format_id| ExpectedStream {
            format_id: format_id.to_owned(),
            size: None,
            approx_size: None,
        })
        .collect();
    let progress = download_progress(NICONICO_SM9_STDOUT);
    let last = progress.len() - 1;
    let mut aggregator = ProgressAggregator::new(expected);

    for (index, update) in progress.iter().enumerate() {
        let fraction = aggregator.update(update).fraction;
        if index == last {
            assert_eq!(fraction, Some(1.0));
        } else {
            assert_eq!(fraction, None, "fraction became known at line {index}");
        }
    }
}

fn download_progress(fixture: &str) -> Vec<vids_core::types::DownloadProgress> {
    parsed_lines(fixture, OutputStream::Stdout)
        .into_iter()
        .filter_map(|line| match line {
            ParsedLine::Download(progress) => Some(progress),
            _ => None,
        })
        .collect()
}
