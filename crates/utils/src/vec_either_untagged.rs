use either::Either;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum EitherUntagged<L, R> {
    Left(L),
    Right(R),
}

pub fn serialize<L, R, S>(this: &[Either<L, R>], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    L: Serialize,
    R: Serialize,
{
    if let [Either::Left(left)] = this {
        return left.serialize(serializer);
    }

    serializer.collect_seq(this.iter().map(|el| match el {
        Either::Left(left) => EitherUntagged::Left(left),
        Either::Right(right) => EitherUntagged::Right(right),
    }))
}

#[derive(Deserialize)]
#[serde(untagged)]
enum VecOrSingle<L, R> {
    Vec(Vec<EitherUntagged<L, R>>),
    Single(L),
}

pub fn deserialize<'de, L, R, D>(deserializer: D) -> Result<Vec<Either<L, R>>, D::Error>
where
    D: Deserializer<'de>,
    L: Deserialize<'de>,
    R: Deserialize<'de>,
{
    VecOrSingle::<L, R>::deserialize(deserializer).map(|input| match input {
        VecOrSingle::Vec(vec) => vec
            .into_iter()
            .map(|el| match el {
                EitherUntagged::Left(left) => Either::Left(left),
                EitherUntagged::Right(right) => Either::Right(right),
            })
            .collect(),
        VecOrSingle::Single(left) => vec![Either::Left(left)],
    })
}

#[cfg(test)]
mod tests {
    use either::Either;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct TestStruct {
        #[serde(with = "super")]
        data: Vec<Either<String, usize>>,
    }

    #[test]
    fn test_serde() {
        let instance = TestStruct {
            data: vec![
                Either::Left("meow".to_string()),
                Either::Right(1874),
                Either::Left("craft".to_string()),
            ],
        };
        let serialized = serde_json::to_string(&instance).unwrap();
        assert_eq!(serialized, r#"{"data":["meow",1874,"craft"]}"#);
        let deserialized = serde_json::from_str::<TestStruct>(&serialized).unwrap();
        assert_eq!(deserialized, instance);
    }

    #[test]
    fn test_single_left_value() {
        let instance = TestStruct {
            data: vec![Either::Left("meow".to_string())],
        };

        let serialized = serde_json::to_string(&instance).unwrap();
        assert_eq!(serialized, r#"{"data":"meow"}"#);

        let deserialized = serde_json::from_str::<TestStruct>(&serialized).unwrap();
        assert_eq!(deserialized, instance);
    }
}
