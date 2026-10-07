use crate::{Policy, VisionError, sealed::Offer, sealed::Realization};

/// Selects one realization for the complete request under `policy`.
pub fn select(
    request: &str,
    policy: Policy,
    native: &Offer,
    portable: &Offer,
) -> Result<Realization, VisionError> {
    match policy {
        Policy::PreferNative => {
            if *native == Offer::Serves {
                return Ok(Realization::Native);
            }
            if *portable == Offer::Serves {
                return Ok(Realization::Portable);
            }
            Err(VisionError::Unsupported(format!(
                "{request}: {}; {}",
                reason(native, Realization::Native),
                reason(portable, Realization::Portable)
            )))
        }
        Policy::PortableOnly => {
            if *portable == Offer::Serves {
                return Ok(Realization::Portable);
            }
            Err(VisionError::Unsupported(format!(
                "{request} under PortableOnly: {}",
                reason(portable, Realization::Portable)
            )))
        }
    }
}

fn reason(offer: &Offer, realization: Realization) -> String {
    match (offer, realization) {
        (Offer::Absent, Realization::Native) => "no native realization on this device".to_owned(),
        (Offer::Absent, Realization::Portable) => {
            "the app does not carry the portable realization".to_owned()
        }
        (Offer::Lacks(feature), realization) => format!(
            "the {} realization lacks {feature}",
            match realization {
                Realization::Native => "native",
                Realization::Portable => "portable",
            }
        ),
        (Offer::Serves, _) => {
            unreachable!("serving offers are never reported as selection failures")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::select;
    use crate::{
        Policy, VisionError,
        sealed::{Offer, Realization},
    };

    #[test]
    fn prefer_native_uses_native_when_both_serve_or_portable_is_absent() {
        assert_eq!(
            select(
                "barcode",
                Policy::PreferNative,
                &Offer::Serves,
                &Offer::Absent
            )
            .unwrap(),
            Realization::Native
        );
        assert_eq!(
            select(
                "barcode",
                Policy::PreferNative,
                &Offer::Serves,
                &Offer::Serves
            )
            .unwrap(),
            Realization::Native
        );
    }

    #[test]
    fn prefer_native_uses_portable_when_native_is_absent_or_lacks_a_feature() {
        assert_eq!(
            select(
                "barcode",
                Policy::PreferNative,
                &Offer::Absent,
                &Offer::Serves
            )
            .unwrap(),
            Realization::Portable
        );
        assert_eq!(
            select(
                "barcode",
                Policy::PreferNative,
                &Offer::Lacks("symbology Aztec".to_owned()),
                &Offer::Serves
            )
            .unwrap(),
            Realization::Portable
        );
    }

    #[test]
    fn prefer_native_reports_both_reasons_when_neither_serves() {
        let error = select(
            "barcode request",
            Policy::PreferNative,
            &Offer::Lacks("symbology Aztec".to_owned()),
            &Offer::Absent,
        )
        .unwrap_err();
        assert!(
            matches!(error, VisionError::Unsupported(message) if message.contains("barcode request") && message.contains("Aztec"))
        );

        let error = select(
            "barcode request",
            Policy::PreferNative,
            &Offer::Absent,
            &Offer::Lacks("x".to_owned()),
        )
        .unwrap_err();
        assert!(matches!(error, VisionError::Unsupported(message) if message.contains('x')));
    }

    #[test]
    fn portable_only_ignores_a_serving_native_realization() {
        assert_eq!(
            select(
                "barcode",
                Policy::PortableOnly,
                &Offer::Serves,
                &Offer::Serves
            )
            .unwrap(),
            Realization::Portable
        );

        let error = select(
            "barcode",
            Policy::PortableOnly,
            &Offer::Serves,
            &Offer::Absent,
        )
        .unwrap_err();
        assert!(
            matches!(error, VisionError::Unsupported(message) if message.contains("barcode") && message.contains("PortableOnly"))
        );
    }
}
