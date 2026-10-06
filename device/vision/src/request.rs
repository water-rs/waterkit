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
    type Output: Send + 'static;
}

macro_rules! impl_request_tuple {
    ($($type:ident:$index:tt:$prepare:ident),+ $(,)?) => {
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
            ) -> impl Future<Output = Result<(), VisionError>> + Send {
                $(
                    let $prepare = self.$index.prepare(context);
                )+
                async move {
                    $(
                        $prepare.await?;
                    )+
                    Ok(())
                }
            }

            fn run(
                self,
                pass: &mut Pass<'_>,
            ) -> impl Future<Output = Result<<($($type,)+) as Request>::Output, VisionError>> + Send {
                async move {
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

impl_request_tuple!(A:0:prepare_a, B:1:prepare_b);
impl_request_tuple!(A:0:prepare_a, B:1:prepare_b, C:2:prepare_c);
impl_request_tuple!(A:0:prepare_a, B:1:prepare_b, C:2:prepare_c, D:3:prepare_d);
impl_request_tuple!(A:0:prepare_a, B:1:prepare_b, C:2:prepare_c, D:3:prepare_d, E:4:prepare_e);
impl_request_tuple!(A:0:prepare_a, B:1:prepare_b, C:2:prepare_c, D:3:prepare_d, E:4:prepare_e, F:5:prepare_f);
impl_request_tuple!(A:0:prepare_a, B:1:prepare_b, C:2:prepare_c, D:3:prepare_d, E:4:prepare_e, F:5:prepare_f, G:6:prepare_g);
impl_request_tuple!(A:0:prepare_a, B:1:prepare_b, C:2:prepare_c, D:3:prepare_d, E:4:prepare_e, F:5:prepare_f, G:6:prepare_g, H:7:prepare_h);
