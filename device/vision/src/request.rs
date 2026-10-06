use std::future::Future;

use crate::{
    VisionError,
    sealed::{Context, Pass, Plan, Sealed},
};

/// A vision operation whose realization is selected before it runs.
///
/// Tuple requests perform each element in order using the same image pass and
/// shared preparations.
pub trait Request: Sealed {
    /// The result produced by this request.
    type Output: wgpu::WasmNotSend + 'static;
}

macro_rules! impl_request_tuple {
    ($($type:ident:$index:tt),+ $(,)?) => {
        impl<$($type: Request),+> Request for ($($type,)+) {
            type Output = ($($type::Output,)+);
        }

        impl<$($type: Request),+> Sealed for ($($type,)+) {
            type Plan = ($($type::Plan,)+);

            fn plan(&self, context: Context<'_>) -> Result<Self::Plan, VisionError> {
                Ok(($(
                    <$type as Sealed>::plan(&self.$index, context)?,
                )+))
            }
        }

        impl<$($type: Request),+> Plan<($($type,)+)> for ($($type::Plan,)+) {
            fn prepare(
                &self,
                context: Context<'_>,
            ) -> impl Future<Output = Result<(), VisionError>> + wgpu::WasmNotSend {
                let prepares = ($(self.$index.prepare(context),)+);
                async move {
                    futures::try_join!($(prepares.$index),+)?;
                    Ok(())
                }
            }

            #[cfg_attr(
                target_arch = "wasm32",
                expect(
                    clippy::future_not_send,
                    reason = "wgpu::WasmNotSend imposes no Send requirement on wasm32"
                )
            )]
            fn run(
                self,
                pass: &mut Pass<'_>,
            ) -> impl Future<
                Output = Result<<($($type,)+) as Request>::Output, VisionError>,
            > + wgpu::WasmNotSend {
                async move {
                    // Sequential: every element runs on this one `&mut Pass`, which owns the shared preparations.
                    Ok((
                        $(
                            self.$index.run(pass).await?,
                        )+
                    ))
                }
            }
        }
    };
}

impl_request_tuple!(A:0, B:1);
impl_request_tuple!(A:0, B:1, C:2);
impl_request_tuple!(A:0, B:1, C:2, D:3);
impl_request_tuple!(A:0, B:1, C:2, D:3, E:4);
impl_request_tuple!(A:0, B:1, C:2, D:3, E:4, F:5);
impl_request_tuple!(A:0, B:1, C:2, D:3, E:4, F:5, G:6);
impl_request_tuple!(A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7);
