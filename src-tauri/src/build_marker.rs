//! 배포 빌드 가드(`scripts/check-release-features.mjs`)가 바이너리에서 찾는 표식.
//! 개발용 feature 에는 따로 고유한 문자열이 없어, 그 feature 를 켠 빌드에서만 시작할 때 실제로 출력해 링커가 지우지 못하게 한다.
//! 이 문자열을 바꾸면 가드 스크립트의 같은 값도 바꿔야 한다(`tests/releaseFeatureGuard.test.ts` 가 일치를 강제한다).
//! `lib.rs` 는 운영 코드에 시험 훅 이름을 두지 않는다는 소스 검사(`tests/api_route.rs`) 대상이라 표식은 이 파일에 둔다

#[cfg(feature = "test-hooks")]
const FEATURE_MARKER_TEST_HOOKS: &str = "antler-build-feature:test-hooks";

/// 켜진 개발용 feature 의 표식을 표준 오류에 낸다(배포 빌드에서는 아무것도 하지 않는다)
pub fn announce() {
    #[cfg(feature = "test-hooks")]
    eprintln!("{FEATURE_MARKER_TEST_HOOKS}");
}
