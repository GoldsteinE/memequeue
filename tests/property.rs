use std::{collections::VecDeque, io::Write as _};

use proptest::{prelude::ProptestConfig, prop_assert_eq, prop_compose, proptest};

use memequeue::MemeQueue;

#[derive(Debug)]
enum Action {
    Read,
    Write(Vec<u8>),
}


proptest! {
    #![proptest_config(ProptestConfig {
        timeout: 100,
        ..ProptestConfig::default()
    })]

    #[test]
    fn simple(actions in proptest::collection::vec(action(), 0..1000)) {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut to_read = VecDeque::new();
        let queue = MemeQueue::from_path(file.path(), 4096).unwrap();
        for action in actions {
            match action {
                Action::Read => {
                    let Some(expected) = to_read.pop_front() else { continue };
                    let data = queue.read(|buf| buf.to_owned());
                    prop_assert_eq!(data, expected);
                },
                Action::Write(buf) => {
                    queue.write(|writer| writer.write_all(&buf)).unwrap();
                    to_read.push_back(buf);
                }
            }
        }
    }
}

prop_compose! {
    fn action()(opt in proptest::option::of(0..40) -> Action {
        match opt {
            None => Action::Read,
            Some(x) => Action::Write(vec![0; x]),
        }
    }
}
