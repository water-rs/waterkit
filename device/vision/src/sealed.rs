use std::{
    any::{Any, TypeId},
    collections::{HashMap, hash_map::Entry},
    fmt,
    future::Future,
    sync::Arc,
};

use crate::{Image, Vision, VisionError, image::Pixels};

/// The implementation contract for sealed vision requests.
pub trait Sealed {
    /// The selected plan for this request.
    type Plan: Plan<Self> + Send + 'static
    where
        Self: crate::Request;

    /// Selects realizations synchronously before execution starts.
    fn plan(&self, context: Context<'_>) -> Result<Self::Plan, VisionError>
    where
        Self: crate::Request;
}

/// A request's selected realizations, ready to run.
pub trait Plan<R: crate::Request + ?Sized> {
    /// Fetches or verifies what the selected realization needs ahead of time.
    fn prepare(&self, context: Context<'_>)
    -> impl Future<Output = Result<(), VisionError>> + Send;

    /// Runs this request using its already selected realization.
    fn run(
        self,
        pass: &mut Pass<'_>,
    ) -> impl Future<Output = Result<R::Output, VisionError>> + Send;
}

/// Whether a realization can serve a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// The realization is not available or not carried.
    Absent,
    /// The realization cannot serve exactly and names the missing requirement.
    Lacks(String),
    /// The realization serves the complete request.
    Serves,
}

/// The selected realization for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Realization {
    /// An operating-system or preinstalled system realization.
    Native,
    /// An application-carried realization.
    Portable,
}

/// Planning context handed to requests.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    vision: &'a Vision,
}

impl<'a> Context<'a> {
    pub(crate) const fn new(vision: &'a Vision) -> Self {
        Self { vision }
    }

    /// The GPU device used for vision work.
    pub const fn device(self) -> &'a Arc<wgpu::Device> {
        &self.vision.device
    }

    /// The GPU queue used for vision work.
    pub const fn queue(self) -> &'a Arc<wgpu::Queue> {
        &self.vision.queue
    }

    /// Chooses the realization serving `request` under the vision policy.
    ///
    /// The selected realization is logged.
    pub fn select(
        self,
        request: &'static str,
        native: &Offer,
        portable: &Offer,
    ) -> Result<Realization, VisionError> {
        let result = crate::selection::select(request, self.vision.policy, native, portable);
        match &result {
            Ok(realization) => tracing::debug!(
                request,
                ?realization,
                policy = ?self.vision.policy,
                "vision realization selected"
            ),
            Err(error) => tracing::debug!(
                request,
                error = %error,
                "no vision realization serves the request"
            ),
        }
        result
    }
}

/// One perform call's image and lazily built shared preparations.
pub struct Pass<'a> {
    context: Context<'a>,
    image: &'a Image,
    prepared: HashMap<TypeId, Box<dyn Any + Send>>,
}

impl fmt::Debug for Pass<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Pass")
            .field("context", &self.context)
            .field("image", &self.image)
            .field("prepared_count", &self.prepared.len())
            .finish_non_exhaustive()
    }
}

impl<'a> Pass<'a> {
    pub(crate) fn new(context: Context<'a>, image: &'a Image) -> Self {
        Self {
            context,
            image,
            prepared: HashMap::new(),
        }
    }

    /// The planning context for this pass.
    pub const fn context(&self) -> Context<'a> {
        self.context
    }

    /// The pixels shared by requests in this pass.
    pub const fn pixels(&self) -> &'a Pixels {
        self.image.pixels()
    }

    /// Builds `P` once per pass and returns the shared preparation.
    pub async fn prepared<P: Preparation>(&mut self) -> Result<&P, VisionError> {
        Ok(match self.prepared.entry(TypeId::of::<P>()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(Box::new(
                P::prepare(self.context, self.image.pixels()).await?,
            )),
        }
        .downcast_ref::<P>()
        .expect("preparations are keyed by their own TypeId"))
    }
}

/// Image preparation shared by requests in one pass.
pub trait Preparation: Send + 'static + Sized {
    /// Prepares image data shared among requests in the pass.
    fn prepare(
        context: Context<'_>,
        pixels: &Pixels,
    ) -> impl Future<Output = Result<Self, VisionError>> + Send;
}
